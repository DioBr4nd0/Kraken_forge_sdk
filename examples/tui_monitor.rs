use crossterm::{
    event::{self, DisableMouseCapture, EnableMouseCapture, Event, KeyCode},
    execute,
    terminal::{EnterAlternateScreen, LeaveAlternateScreen, disable_raw_mode, enable_raw_mode},
};
use forge_sdk::KrakenClient;
use forge_sdk::book::checksum::validate_checksum;
use forge_sdk::book::orderbook::LocalBook;
use forge_sdk::model::message::{BookData, BookLevel, BookMessage, KrakenMessage};
use memchr::memmem;
use once_cell::sync::Lazy;
use ratatui::{prelude::*, widgets::*};
use regex::Regex;
use std::{
    collections::HashMap,
    io,
    sync::{Arc, Mutex},
    time::{Duration, Instant},
};

// --- STATIC REGEX COMPILATION (O(1) PERFORMANCE) ---
static CHANNEL_RE: Lazy<Regex> = Lazy::new(|| Regex::new(r#""channel":"([^"]+)""#).unwrap());
static TYPE_RE: Lazy<Regex> = Lazy::new(|| Regex::new(r#""type":"([^"]+)""#).unwrap());
static SYMBOL_RE: Lazy<Regex> = Lazy::new(|| Regex::new(r#""symbol":"([^"]+)""#).unwrap());
static CHECKSUM_RE: Lazy<Regex> = Lazy::new(|| Regex::new(r#""checksum":(\d+)"#).unwrap());
static BOOK_LEVEL_RE: Lazy<Regex> =
    Lazy::new(|| Regex::new(r#"\{"price":([\d.]+),"qty":([\d.]+)\}"#).unwrap());

// --- CONSTANTS ---
const TRADING_PAIRS: &[&str] = &["BTC/USD", "ETH/USD", "SOL/USD"];
const DEFAULT_ORDERBOOK_DEPTH: usize = 10;
const MAX_LOG_ENTRIES: usize = 50;
const UI_POLL_INTERVAL_MS: u64 = 50;
const WHALE_ALERT_THRESHOLD: f64 = 50_000.0;
const MEDIUM_TRADE_THRESHOLD: f64 = 10_000.0;

// --- CHECKSUM VALIDATION CONSTANTS ---
const CHECKSUM_GRACE_PERIOD_SECS: u64 = 3; // Skip validation for 3s after snapshot
const MAX_CHECKSUM_FAILURES: u32 = 3; // Failures before triggering recovery

/// Manually parse book message using pre-compiled static regexes (O(1) compilation)
fn manually_parse_book_message(json: &str) -> Result<KrakenMessage, serde_json::Error> {
    let channel = CHANNEL_RE
        .captures(json)
        .and_then(|c| c.get(1))
        .map(|m| m.as_str().to_string())
        .unwrap_or_default();

    let type_str = TYPE_RE
        .captures(json)
        .and_then(|c| c.get(1))
        .map(|m| m.as_str().to_string())
        .unwrap_or_default();

    let symbol = SYMBOL_RE
        .captures(json)
        .and_then(|c| c.get(1))
        .map(|m| m.as_str().to_string())
        .unwrap_or_default();

    let checksum: u32 = CHECKSUM_RE
        .captures(json)
        .and_then(|c| c.get(1))
        .and_then(|m| m.as_str().parse().ok())
        .unwrap_or(0);

    let bids = extract_price_qty_array_from_raw(json, "bids");
    let asks = extract_price_qty_array_from_raw(json, "asks");

    let book_data = BookData {
        symbol,
        checksum,
        bids,
        asks,
    };

    let book_msg = BookMessage {
        channel,
        r#type: type_str,
        data: vec![book_data],
    };

    Ok(KrakenMessage::Book(book_msg))
}

/// Extract price/quantity array using memchr for O(1) compilation + fast search
fn extract_price_qty_array_from_raw(json: &str, field_name: &str) -> Vec<BookLevel> {
    let pattern = format!("\"{}\":[", field_name);
    let bytes = json.as_bytes();

    let finder = memmem::Finder::new(pattern.as_bytes());

    let start = match finder.find(bytes) {
        Some(pos) => pos + pattern.len(),
        None => return vec![],
    };

    let mut depth = 0;
    let mut end = start;
    for i in start..bytes.len() {
        match bytes[i] {
            b'[' => depth += 1,
            b']' => {
                if depth == 0 {
                    end = i;
                    break;
                }
                depth -= 1;
            }
            _ => {}
        }
    }

    let array_content = &json[start..end];

    BOOK_LEVEL_RE
        .captures_iter(array_content)
        .map(|cap| BookLevel {
            price: cap[1].to_string(),
            qty: cap[2].to_string(),
        })
        .collect()
}

// --- STATE MANAGEMENT ---
#[derive(PartialEq, Clone, Copy)]
enum SyncStatus {
    Healthy,
    ChecksumFail,
    Recovering,
    NetworkLost,
}

/// Per-pair synchronization state for robust checksum validation
struct BookState {
    pub book: LocalBook,
    /// When we last received a snapshot (for grace period)
    pub last_snapshot: Option<Instant>,
    /// Number of consecutive checksum failures
    pub checksum_failures: u32,
    /// Is this pair currently recovering?
    pub recovering: bool,
}

impl BookState {
    fn new() -> Self {
        Self {
            book: LocalBook::new(),
            last_snapshot: None,
            checksum_failures: 0,
            recovering: false,
        }
    }

    /// Check if we're within the grace period after a snapshot
    fn in_grace_period(&self) -> bool {
        if let Some(snapshot_time) = self.last_snapshot {
            snapshot_time.elapsed().as_secs() < CHECKSUM_GRACE_PERIOD_SECS
        } else {
            true // No snapshot yet, treat as grace period
        }
    }
}

struct AppState {
    pub books: HashMap<String, BookState>,
    pub logs: Vec<String>,
    pub status: SyncStatus,
    pub last_checksum: u32,
    pub curr_index: usize,
    pub pairs: Vec<String>,
    pub frozen_view: Option<LocalBook>,
    pub metrics: Metrics,
    /// Which pair is currently recovering (for display)
    pub recovering_pair: Option<String>,
}

struct Metrics {
    pub total_messages: u64,
    pub start_time: Instant,
    pub mps: u64,
    pub last_second_count: u64,
    pub last_tick: Instant,
    pub processing_latency: Duration,
    pub last_message_time: Instant,
}

impl Metrics {
    fn new() -> Self {
        let now = Instant::now();
        Self {
            total_messages: 0,
            start_time: now,
            mps: 0,
            last_second_count: 0,
            last_tick: now,
            processing_latency: Duration::from_micros(0),
            last_message_time: now,
        }
    }
}

impl AppState {
    fn new() -> Self {
        Self {
            books: HashMap::new(),
            logs: vec!["Initializing Kraken ...".to_string()],
            status: SyncStatus::Healthy,
            last_checksum: 0,
            curr_index: 0,
            pairs: TRADING_PAIRS.iter().map(|s| s.to_string()).collect(),
            frozen_view: None,
            metrics: Metrics::new(),
            recovering_pair: None,
        }
    }

    /// Calculate overall status based on per-pair states
    fn calculate_status(&mut self) {
        // Check if any pair is recovering
        for (pair, state) in &self.books {
            if state.recovering {
                self.status = SyncStatus::Recovering;
                self.recovering_pair = Some(pair.clone());
                return;
            }
        }

        // All pairs healthy
        self.status = SyncStatus::Healthy;
        self.recovering_pair = None;
    }

    fn log(&mut self, msg: String) {
        self.logs.push(msg);
        if self.logs.len() > MAX_LOG_ENTRIES {
            self.logs.remove(0);
        }
    }
}

// --- SUBSCRIPTION MANAGEMENT ---
fn build_subscription_command(channel: &str, symbol: &str, depth: Option<usize>) -> String {
    if let Some(d) = depth {
        format!(
            r#"{{"method":"subscribe", "params":{{"channel":"{}", "symbol":["{}"], "depth":{}}}}}"#,
            channel, symbol, d
        )
    } else {
        format!(
            r#"{{"method":"subscribe", "params":{{"channel":"{}", "symbol":["{}"]}}}}"#,
            channel, symbol
        )
    }
}

async fn subscribe_to_channels(client: &mut KrakenClient) {
    for pair in TRADING_PAIRS {
        let book_cmd = build_subscription_command("book", pair, Some(DEFAULT_ORDERBOOK_DEPTH));
        let trade_cmd = build_subscription_command("trade", pair, None);
        let ohlc_cmd = format!(
            r#"{{"method":"subscribe", "params":{{"channel":"ohlc", "symbol":["{}"], "interval":1}}}}"#,
            pair
        );

        let _ = client.send_raw(book_cmd).await;
        let _ = client.send_raw(trade_cmd).await;
        let _ = client.send_raw(ohlc_cmd).await;
    }
}

/// Re-subscribe to a single pair's book feed (for recovery)
async fn resubscribe_pair(client: &mut KrakenClient, pair: &str) {
    // Unsubscribe first
    let unsub_cmd = format!(
        r#"{{"method":"unsubscribe", "params":{{"channel":"book", "symbol":["{}"]}}}}"#,
        pair
    );
    let _ = client.send_raw(unsub_cmd).await;

    // Small delay to ensure unsubscribe processes
    tokio::time::sleep(Duration::from_millis(100)).await;

    // Re-subscribe to get fresh snapshot
    let sub_cmd = build_subscription_command("book", pair, Some(DEFAULT_ORDERBOOK_DEPTH));
    let _ = client.send_raw(sub_cmd).await;
}

// --- MAIN ENTRY ---
#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    rustls::crypto::ring::default_provider()
        .install_default()
        .ok();

    enable_raw_mode()?;
    let mut stdout = io::stdout();
    execute!(stdout, EnterAlternateScreen, EnableMouseCapture)?;
    let backend = CrosstermBackend::new(stdout);
    let mut terminal = Terminal::new(backend)?;

    let app_state = Arc::new(Mutex::new(AppState::new()));

    let state_clone = app_state.clone();
    tokio::spawn(async move {
        run_network_loop(state_clone).await;
    });

    let res = run_ui_loop(&mut terminal, app_state);

    disable_raw_mode()?;
    execute!(
        terminal.backend_mut(),
        LeaveAlternateScreen,
        DisableMouseCapture
    )?;
    terminal.show_cursor()?;

    if let Err(err) = res {
        println!("{:?}", err)
    }
    Ok(())
}

// --- NETWORK LOOP WITH METRICS & SELF-HEALING ---
async fn run_network_loop(state: Arc<Mutex<AppState>>) {
    let mut client = match KrakenClient::connect("wss://ws.kraken.com/v2").await {
        Ok(c) => c,
        Err(e) => {
            state
                .lock()
                .unwrap()
                .log(format!("Connection Error: {}", e));
            return;
        }
    };

    subscribe_to_channels(&mut client).await;

    let mut stream = client.stream();
    state
        .lock()
        .unwrap()
        .log("Connected & Subscribed.".to_string());

    // Simple non-blocking message loop - UI will detect stale data
    while let Some(msg_res) = stream.recv().await {
        match msg_res {
            Ok(txt) => {
                // ⏱️ START PERFORMANCE TRACKING
                let start_process = Instant::now();

                // Parse message
                let parsed = if txt.contains("\"channel\":\"book\"") {
                    manually_parse_book_message(&txt)
                } else {
                    serde_json::from_str::<KrakenMessage>(&txt)
                };

                if let Ok(parsed) = parsed {
                    let mut pair_to_recover: Option<String> = None;

                    {
                        let mut s = state.lock().unwrap();

                        // 📊 UPDATE MESSAGE COUNTER & LAST MESSAGE TIME
                        s.metrics.total_messages += 1;
                        s.metrics.last_message_time = start_process;

                        // Restore healthy status if was network lost
                        if s.status == SyncStatus::NetworkLost {
                            s.status = SyncStatus::Healthy;
                            s.log("✅ Network connection restored!".to_string());
                        }

                        // 🚀 CALCULATE MESSAGES PER SECOND (MPS)
                        if start_process.duration_since(s.metrics.last_tick).as_secs() >= 1 {
                            let current = s.metrics.total_messages;
                            let previous = s.metrics.last_second_count;

                            s.metrics.mps = current - previous;
                            s.metrics.last_second_count = current;
                            s.metrics.last_tick = start_process;
                        }

                        // Process message types
                        match parsed {
                            KrakenMessage::Book(msg) => {
                                pair_to_recover = handle_book_message(&mut s, msg);
                            }
                            KrakenMessage::Trade(msg) => {
                                handle_trade_message(&mut s, msg);
                            }
                            KrakenMessage::OHLC(_) => {} // Handled by chart_viewer example
                            KrakenMessage::Heartbeat { .. } => {}
                            _ => {}
                        }

                        // ⚡ MEASURE PROCESSING LATENCY
                        let elapsed = start_process.elapsed();
                        s.metrics.processing_latency = elapsed;
                    }

                    // Handle per-pair recovery OUTSIDE the lock
                    if let Some(pair) = pair_to_recover {
                        resubscribe_pair(&mut client, &pair).await;
                    }
                }
            }
            Err(_e) => {
                // WebSocket error - mark as network lost
                let mut s = state.lock().unwrap();
                s.status = SyncStatus::NetworkLost;
                s.log("🔌 WebSocket error - connection issue".to_string());
            }
        }
    }
}

/// Handle book message updates with per-pair checksum validation
fn handle_book_message(state: &mut AppState, msg: BookMessage) -> Option<String> {
    let data = &msg.data[0];
    let sym = data.symbol.clone();

    // Ensure BookState exists for this pair
    state
        .books
        .entry(sym.clone())
        .or_insert_with(BookState::new);

    let mut pair_to_recover: Option<String> = None;
    let mut log_msg: Option<String> = None;

    if msg.r#type == "snapshot" {
        // Handle snapshot - reset sync state for this pair
        if let Some(book_state) = state.books.get_mut(&sym) {
            book_state
                .book
                .apply_snapshot(data.bids.clone(), data.asks.clone());
            book_state.last_snapshot = Some(Instant::now());
            book_state.checksum_failures = 0;
            book_state.recovering = false;
            log_msg = Some(format!("📸 Snapshot Loaded for {}", sym));
        }
    } else {
        // Handle delta update
        if let Some(book_state) = state.books.get_mut(&sym) {
            // Always apply updates
            book_state.book.apply_updates(data.bids.clone(), true);
            book_state.book.apply_updates(data.asks.clone(), false);

            // Only validate checksum if:
            // 1. Not in grace period (have snapshot and it's been > 3 seconds)
            // 2. Not currently recovering
            if !book_state.in_grace_period() && !book_state.recovering {
                let is_valid = validate_checksum(&book_state.book, data.checksum);

                if !is_valid {
                    book_state.checksum_failures += 1;

                    if book_state.checksum_failures >= MAX_CHECKSUM_FAILURES {
                        // Too many failures - trigger recovery for THIS pair only
                        log_msg = Some(format!(
                            "❌ {} checksum failures for {} - recovering...",
                            book_state.checksum_failures, sym
                        ));
                        book_state.recovering = true;
                        book_state.book = LocalBook::new();
                        pair_to_recover = Some(sym.clone());
                    }
                } else {
                    // Checksum valid - reset failure count
                    book_state.checksum_failures = 0;
                }
            }
        }
    }

    // Log message AFTER releasing the mutable borrow
    if let Some(msg) = log_msg {
        state.log(msg);
    }

    // Store last checksum for display
    state.last_checksum = data.checksum;

    // Update overall status
    state.calculate_status();

    pair_to_recover
}

/// Handle trade messages with whale detection
fn handle_trade_message(state: &mut AppState, msg: forge_sdk::model::trade::TradeMessage) {
    for t in msg.data {
        let val = t.price * t.qty;
        if val > WHALE_ALERT_THRESHOLD {
            state.log(format!(
                "🚨 WHALE ALERT: {} sold {:.4} {} (${:.0})",
                t.side.to_uppercase(),
                t.qty,
                t.symbol,
                val
            ));
        } else if val > MEDIUM_TRADE_THRESHOLD {
            state.log(format!("💰 Trade: {} {} ${:.0}", t.symbol, t.side, t.price));
        }
    }
}

// --- UI LOOP ---
fn run_ui_loop(
    terminal: &mut Terminal<CrosstermBackend<io::Stdout>>,
    state: Arc<Mutex<AppState>>,
) -> io::Result<()> {
    loop {
        // Check if data is stale (no message in 5 seconds = network issue)
        {
            let mut s = state.lock().unwrap();
            let time_since_last_msg = s.metrics.last_message_time.elapsed();
            if time_since_last_msg > Duration::from_secs(5) && s.status != SyncStatus::NetworkLost {
                s.status = SyncStatus::NetworkLost;
                s.log("🔌 Network Lost - No data for 5 seconds".to_string());
            }
        }

        terminal.draw(|f| ui(f, &state))?;

        if event::poll(Duration::from_millis(UI_POLL_INTERVAL_MS))? {
            if let Event::Key(key) = event::read()? {
                match key.code {
                    KeyCode::Tab => {
                        let mut s = state.lock().unwrap();
                        s.curr_index = (s.curr_index + 1) % s.pairs.len();
                    }
                    KeyCode::Char(' ') => {
                        let mut s = state.lock().unwrap();
                        if s.frozen_view.is_some() {
                            s.frozen_view = None;
                        } else {
                            let current_pair = &s.pairs[s.curr_index];
                            if let Some(book_state) = s.books.get(current_pair) {
                                s.frozen_view = Some(book_state.book.clone());
                            }
                        }
                    }
                    KeyCode::Char('q') => {
                        return Ok(());
                    }
                    _ => {}
                }
            }
        }
    }
}

// --- UI RENDERING ---
fn ui(f: &mut Frame, state: &Arc<Mutex<AppState>>) {
    let s = state.lock().unwrap();

    let chunks = Layout::default()
        .direction(Direction::Vertical)
        .constraints([
            Constraint::Length(3),
            Constraint::Min(1),
            Constraint::Length(3),
        ])
        .split(f.area());

    let main_chunks = Layout::default()
        .direction(Direction::Horizontal)
        .constraints([Constraint::Percentage(50), Constraint::Percentage(50)])
        .split(chunks[1]);

    let current_pair = &s.pairs[s.curr_index];

    render_header(f, chunks[0], current_pair);
    render_order_book(f, main_chunks[0], &s, current_pair);
    render_logs(f, main_chunks[1], &s);
    render_footer(f, chunks[2], &s);
}

fn render_header(f: &mut Frame, area: Rect, pair: &str) {
    let title_text = format!(
        "🐙 KRAKEN FORGE SDK TERMINAL [{}] (Depth: {}) 🐙",
        pair, DEFAULT_ORDERBOOK_DEPTH
    );
    let title = Paragraph::new(title_text)
        .style(
            Style::default()
                .fg(Color::Cyan)
                .add_modifier(Modifier::BOLD),
        )
        .alignment(Alignment::Center)
        .block(Block::default().borders(Borders::ALL));
    f.render_widget(title, area);
}

fn render_order_book(f: &mut Frame, area: Rect, state: &AppState, pair: &str) {
    let mut rows = Vec::new();
    rows.push(
        Row::new(vec!["Side", "Price", "Qty"])
            .style(Style::default().add_modifier(Modifier::UNDERLINED)),
    );

    // Get the book to render - either frozen view or live from BookState
    let book_to_render: Option<&LocalBook> = state
        .frozen_view
        .as_ref()
        .or_else(|| state.books.get(pair).map(|bs| &bs.book));

    if let Some(book) = book_to_render {
        for (_price, (p_str, q_str)) in book.asks.iter().take(DEFAULT_ORDERBOOK_DEPTH) {
            rows.push(Row::new(vec![
                Cell::from("ASK").style(Style::default().fg(Color::Red)),
                Cell::from(p_str.as_str()),
                Cell::from(q_str.as_str()),
            ]));
        }

        rows.push(Row::new(vec!["---", "---", "---"]));

        for (_price, (p_str, q_str)) in book.bids.iter().rev().take(DEFAULT_ORDERBOOK_DEPTH) {
            rows.push(Row::new(vec![
                Cell::from("BID").style(Style::default().fg(Color::Green)),
                Cell::from(p_str.as_str()),
                Cell::from(q_str.as_str()),
            ]));
        }
    } else {
        rows.push(Row::new(vec![
            Cell::from("waiting data...").style(Style::default().fg(Color::DarkGray)),
            Cell::from("..."),
            Cell::from("..."),
        ]));
    }

    let book_table = Table::new(
        rows,
        [
            Constraint::Percentage(20),
            Constraint::Percentage(40),
            Constraint::Percentage(40),
        ],
    )
    .block(
        Block::default()
            .title(format!(" Order Book ({}) ", pair))
            .borders(Borders::ALL),
    );
    f.render_widget(book_table, area);
}

fn render_logs(f: &mut Frame, area: Rect, state: &AppState) {
    let log_items: Vec<ListItem> = state
        .logs
        .iter()
        .rev()
        .take(20)
        .map(|msg| {
            let style = if msg.contains("WHALE") {
                Style::default()
                    .fg(Color::Yellow)
                    .add_modifier(Modifier::BOLD | Modifier::RAPID_BLINK)
            } else if msg.contains("Mismatch") || msg.contains("Fail") {
                Style::default().fg(Color::Red).bg(Color::White)
            } else if msg.contains("Recovering") {
                Style::default().fg(Color::Yellow)
            } else {
                Style::default().fg(Color::White)
            };
            ListItem::new(msg.clone()).style(style)
        })
        .collect();

    let logs_list = List::new(log_items).block(
        Block::default()
            .title(" Live Event Feed ")
            .borders(Borders::ALL),
    );
    f.render_widget(logs_list, area);
}

fn render_footer(f: &mut Frame, area: Rect, state: &AppState) {
    let uptime_secs = state.metrics.start_time.elapsed().as_secs();
    let latency_micros = state.metrics.processing_latency.as_micros();

    let (status_indicator, status_style) = match state.status {
        SyncStatus::Healthy => ("✅ Checksum Valid", Style::default().fg(Color::Green)),
        SyncStatus::ChecksumFail => (
            "❌ Checksum Mismatch",
            Style::default().fg(Color::Red).add_modifier(Modifier::BOLD),
        ),
        SyncStatus::Recovering => (
            "⚠️ Recovering...",
            Style::default()
                .fg(Color::Yellow)
                .add_modifier(Modifier::RAPID_BLINK),
        ),
        SyncStatus::NetworkLost => (
            "🔌 Network Lost - Reconnecting...",
            Style::default()
                .fg(Color::Red)
                .add_modifier(Modifier::RAPID_BLINK),
        ),
    };

    let footer_text = format!(
        " {} | 🚀 {} msg/s | ⚡ {}µs | 📊 {} msgs | ⏱️ {}s ",
        status_indicator,
        state.metrics.mps,
        latency_micros,
        state.metrics.total_messages,
        uptime_secs
    );

    let footer = Paragraph::new(footer_text)
        .style(status_style)
        .block(Block::default().borders(Borders::ALL));
    f.render_widget(footer, area);
}
