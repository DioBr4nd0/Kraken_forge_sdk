use crossterm::{
    event::{self, DisableMouseCapture, EnableMouseCapture, Event, KeyCode},
    execute,
    terminal::{EnterAlternateScreen, LeaveAlternateScreen, disable_raw_mode, enable_raw_mode},
};
use forge_sdk::KrakenClient;
use forge_sdk::book::checksum::validate_checksum;
use forge_sdk::book::orderbook::LocalBook;
use forge_sdk::model::message::{BookData, BookLevel, BookMessage, KrakenMessage};
use ratatui::{prelude::*, widgets::*};
use regex::Regex;
use std::{
    collections::HashMap,
    io,
    sync::{Arc, Mutex},
    time::Duration,
};

fn manually_parse_book_message(json: &str) -> Result<KrakenMessage, serde_json::Error> {
    let channel_re = Regex::new(r#""channel":"([^"]+)""#).unwrap();
    let type_re = Regex::new(r#""type":"([^"]+)""#).unwrap();
    let symbol_re = Regex::new(r#""symbol":"([^"]+)""#).unwrap();
    let checksum_re = Regex::new(r#""checksum":(\d+)"#).unwrap();

    let channel = channel_re
        .captures(json)
        .and_then(|c| c.get(1))
        .map(|m| m.as_str().to_string())
        .unwrap_or_default();
    let type_str = type_re
        .captures(json)
        .and_then(|c| c.get(1))
        .map(|m| m.as_str().to_string())
        .unwrap_or_default();
    let symbol = symbol_re
        .captures(json)
        .and_then(|c| c.get(1))
        .map(|m| m.as_str().to_string())
        .unwrap_or_default();
    let checksum: u32 = checksum_re
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

fn extract_price_qty_array_from_raw(json: &str, field_name: &str) -> Vec<BookLevel> {
    let array_pattern = format!(r#""{}":\[(.*?)\]"#, field_name);
    let array_re = Regex::new(&array_pattern).unwrap();

    let array_content = array_re    
        .captures(json)
        .and_then(|c| c.get(1))
        .map(|m| m.as_str())
        .unwrap_or("");

    let obj_re = Regex::new(r#"\{"price":([\d.]+),"qty":([\d.]+)\}"#).unwrap();

    obj_re
        .captures_iter(array_content)
        .map(|cap| {
            let price = cap.get(1).unwrap().as_str().to_string();
            let qty = cap.get(2).unwrap().as_str().to_string();
            BookLevel { price, qty }
        })
        .collect()
}

// --- UPGRADED STATE MANAGEMENT ---
#[derive(PartialEq, Clone, Copy)]
enum SyncStatus {
    Healthy,
    ChecksumFail,
    Recovering,
}

struct AppState {
    pub books: HashMap<String, LocalBook>,
    pub logs: Vec<String>,
    pub status: SyncStatus, // Replaced simple boolean with Enum
    pub last_checksum: u32,
    pub curr_index: usize,
    pub pairs: Vec<String>,
    pub frozen_view: Option<LocalBook>,
    pub depth: usize,
}

impl AppState {
    fn new() -> Self {
        let mut _pairs = vec![];
        _pairs.push("BTC/USD".to_string());
        _pairs.push("ETH/USD".to_string());
        _pairs.push("SOL/USD".to_string());
        Self {
            books: HashMap::new(),
            logs: vec!["Initializing Kraken ...".to_string()],
            status: SyncStatus::Healthy,
            last_checksum: 0,
            curr_index: 0,
            pairs: _pairs,
            frozen_view: None,
            depth: 10,
        }
    }

    fn log(&mut self, msg: String) {
        self.logs.push(msg);
        if self.logs.len() > 50 {
            self.logs.remove(0);
        }
    }
}

// --- MAIN ENTRY ---
#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    // 1. Setup Crypto Provider
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

// --- UPGRADED NETWORK LOOP (SELF-HEALING) ---
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

    // Define commands
    let book_sub_cmd1 =
        r#"{"method":"subscribe", "params":{"channel":"book", "symbol":["BTC/USD"], "depth":10}}"#
            .to_string();
    let book_sub_cmd2 =
        r#"{"method":"subscribe", "params":{"channel":"book", "symbol":["ETH/USD"], "depth":10}}"#
            .to_string();
    let book_sub_cmd3 =
        r#"{"method":"subscribe", "params":{"channel":"book", "symbol":["SOL/USD"], "depth":10}}"#
            .to_string();
    let trade_sub_cmd1 =
        r#"{"method":"subscribe", "params":{"channel":"trade", "symbol":["BTC/USD"]}}"#.to_string();
    let trade_sub_cmd2 =
        r#"{"method":"subscribe", "params":{"channel":"trade", "symbol":["ETH/USD"]}}"#.to_string();
    let trade_sub_cmd3 =
        r#"{"method":"subscribe", "params":{"channel":"trade", "symbol":["SOL/USD"]}}"#.to_string();

    // Use send_raw so the Engine 'remembers' these for auto-reconnect
    let _ = client.send_raw(book_sub_cmd1.clone()).await;
    let _ = client.send_raw(book_sub_cmd2.clone()).await;
    let _ = client.send_raw(book_sub_cmd3.clone()).await;

    let _ = client.send_raw(trade_sub_cmd1).await;
    let _ = client.send_raw(trade_sub_cmd2).await;
    let _ = client.send_raw(trade_sub_cmd3).await;

    let mut stream = client.stream();
    state
        .lock()
        .unwrap()
        .log("Connected & Subscribed.".to_string());

    while let Some(msg_res) = stream.recv().await {
        if let Ok(txt) = msg_res {
            // Your custom parsing logic
            let parsed = if txt.contains("\"channel\":\"book\"") {
                manually_parse_book_message(&txt)
            } else {
                serde_json::from_str::<KrakenMessage>(&txt)
            };

            if let Ok(parsed) = parsed {
                let mut should_recover = false;

                {
                    let mut s = state.lock().unwrap();

                    match parsed {
                        KrakenMessage::Book(msg) => {
                            let data = &msg.data[0];

                            // HEALING LOGIC START
                            let sym = &data.symbol;
                            // Ensure entry exists (requires mutable borrow of s)
                            s.books.entry(sym.clone()).or_insert_with(LocalBook::new);

                            let mut validation_result: Option<(bool, u32)> = None;
                            let mut snapshot_applied = false;

                            if msg.r#type == "snapshot" {
                                if let Some(book) = s.books.get_mut(sym) {
                                    book.apply_snapshot(data.bids.clone(), data.asks.clone());
                                    snapshot_applied = true;
                                }
                            } else {
                                if s.status != SyncStatus::Recovering {
                                    if let Some(book) = s.books.get_mut(sym) {
                                        book.apply_updates(data.bids.clone(), true);
                                        book.apply_updates(data.asks.clone(), false);

                                        let is_valid = validate_checksum(book, data.checksum);
                                        validation_result = Some((is_valid, data.checksum));
                                    }
                                }
                            }

                            // Now 'book' borrow is gone, we can use 's' mutably again
                            if snapshot_applied {
                                s.log(format!("📸 Snapshot Loaded for {} (Sync Restored)", sym));
                                s.status = SyncStatus::Healthy;
                            }

                            if let Some((is_valid, checksum)) = validation_result {
                                s.last_checksum = checksum;
                                if !is_valid {
                                    s.status = SyncStatus::ChecksumFail;
                                    s.log(format!(
                                        "❌ Checksum Fail for {}: {}. Triggering Auto-Recovery...",
                                        sym, checksum
                                    ));

                                    // TRIGGER RECOVERY:
                                    s.status = SyncStatus::Recovering;
                                    // Clear corrupted book
                                    if let Some(book) = s.books.get_mut(sym) {
                                        *book = LocalBook::new();
                                    }
                                    should_recover = true;
                                }
                            }
                            // HEALING LOGIC END
                        }
                        KrakenMessage::Trade(msg) => {
                            for t in msg.data {
                                let val = t.price * t.qty;
                                // Whale Alert Logic > $50k
                                if val > 50_000.0 {
                                    s.log(format!(
                                        "🚨 WHALE ALERT: {} sold {:.4} {} (${:.0})",
                                        t.side.to_uppercase(),
                                        t.qty,
                                        t.symbol,
                                        val
                                    ));
                                } else if val > 10_000.0 {
                                    // Log medium trades
                                    s.log(format!(
                                        "💰 Trade: {} {} ${:.0}",
                                        t.symbol, t.side, t.price
                                    ));
                                }
                            }
                        }
                        KrakenMessage::Heartbeat { .. } => {}
                        _ => {}
                    }
                } // MutexGuard dropped here

                if should_recover {
                    let _ = client.send_raw(book_sub_cmd1.clone()).await;
                    let _ = client.send_raw(book_sub_cmd2.clone()).await;
                    let _ = client.send_raw(book_sub_cmd3.clone()).await;
                }
            }
        }
    }
}

// --- UI LOOP (Kept Same) ---
fn run_ui_loop(
    terminal: &mut Terminal<CrosstermBackend<io::Stdout>>,
    state: Arc<Mutex<AppState>>,
) -> io::Result<()> {
    loop {
        terminal.draw(|f| ui(f, &state))?;

        if event::poll(Duration::from_millis(50))? {
            if let Event::Key(key) = event::read()? {
                match key.code {
                    KeyCode::Tab => {
                        let mut s = state.lock().unwrap();
                        s.curr_index += 1;
                        if s.curr_index >= s.books.len() {
                            s.curr_index = 0;
                        }
                    }
                    KeyCode::Char(' ') => {
                        let mut s = state.lock().unwrap();
                        if s.frozen_view.is_some() {
                            s.frozen_view = None;
                        } else {
                            let current_pair = &s.pairs[s.curr_index];
                            if let Some(live_book) = s.books.get(current_pair) {
                                s.frozen_view = Some(live_book.clone());
                            }
                        }
                    }
                    KeyCode::Char('q') => {
                        return Ok(());
                    }
                    KeyCode::Char('+') | KeyCode::Char('=') => {
                        let mut s = state.lock().unwrap();
                        s.depth = (s.depth + 1).min(50);
                    }
                    KeyCode::Char('-') | KeyCode::Char('_') => {
                        let mut s = state.lock().unwrap();
                        s.depth = s.depth.saturating_sub(1).max(1);
                    }
                    _ => {}
                }
            }
        }
    }
}

// --- UPGRADED UI RENDER ---
fn ui(f: &mut Frame, state: &Arc<Mutex<AppState>>) {
    let s = state.lock().unwrap();

    // 1. Layout: Header (Top), Main (Middle), Footer (Bottom)
    let chunks = Layout::default()
        .direction(Direction::Vertical)
        .constraints([
            Constraint::Length(3), // Header
            Constraint::Min(1),    // Content
            Constraint::Length(3), // Footer
        ])
        .split(f.area());

    // 2. Main Content Split: Book (Left), Logs (Right)
    let main_chunks = Layout::default()
        .direction(Direction::Horizontal)
        .constraints([Constraint::Percentage(50), Constraint::Percentage(50)])
        .split(chunks[1]);

    let current_pair = &s.pairs[s.curr_index];
    let depth = s.depth;

    let title_text = format!(
        "🐙 KRAKEN FORGE SDK TERMINAL [{}] (Depth: {}) 🐙",
        current_pair, depth
    );
    // --- WIDGET 1: HEADER ---
    let title = Paragraph::new(title_text)
        .style(
            Style::default()
                .fg(Color::Cyan)
                .add_modifier(Modifier::BOLD),
        )
        .alignment(Alignment::Center)
        .block(Block::default().borders(Borders::ALL));
    f.render_widget(title, chunks[0]);

    // --- WIDGET 2: ORDER BOOK (LEFT) ---
    // Prepare rows
    let mut rows = Vec::new();
    rows.push(
        Row::new(vec!["Side", "Price", "Qty"])
            .style(Style::default().add_modifier(Modifier::UNDERLINED)),
    );

    let book_to_render = if let Some(frozen) = &s.frozen_view {
        Some(frozen)
    } else {
        s.books.get(current_pair)
    };

    if let Some(book) = book_to_render {
        for (_price, (p_str, q_str)) in book.asks.iter().take(depth) {
            rows.push(Row::new(vec![
                Cell::from("ASK").style(Style::default().fg(Color::Red)),
                Cell::from(p_str.as_str()),
                Cell::from(q_str.as_str()),
            ]));
        }

        rows.push(Row::new(vec!["---", "---", "---"]));

        for (_price, (p_str, q_str)) in book.bids.iter().rev().take(depth) {
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
            .title(format!(" Order Book ({}) ", current_pair))
            .borders(Borders::ALL),
    );
    f.render_widget(book_table, main_chunks[0]);

    // --- WIDGET 3: LOGS ---
    let log_items: Vec<ListItem> = s
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
    f.render_widget(logs_list, main_chunks[1]);

    // --- WIDGET 4: FOOTER (UPDATED FOR HEALING STATUS) ---
    let (status_text, status_style) = match s.status {
        SyncStatus::Healthy => (
            format!("Checksum Status: ✅ VALID | Last RPC: {}", s.last_checksum),
            Style::default().fg(Color::Green),
        ),
        SyncStatus::ChecksumFail => (
            format!(
                "Checksum Status: ❌ FAILURE | Last RPC: {}",
                s.last_checksum
            ),
            Style::default().fg(Color::Red).add_modifier(Modifier::BOLD),
        ),
        SyncStatus::Recovering => (
            format!("Checksum Status: ⚠️ RECOVERING (Resyncing)..."),
            Style::default()
                .fg(Color::Yellow)
                .add_modifier(Modifier::RAPID_BLINK),
        ),
    };

    let footer = Paragraph::new(status_text)
        .style(status_style)
        .block(Block::default().borders(Borders::ALL));
    f.render_widget(footer, chunks[2]);
}
