//! Candlestick Chart Viewer Example
//!
//! A dedicated TUI for viewing OHLC candlestick price data.
//! Run with: cargo run --example chart_viewer

use crossterm::{
    event::{self, DisableMouseCapture, EnableMouseCapture, Event, KeyCode},
    execute,
    terminal::{EnterAlternateScreen, LeaveAlternateScreen, disable_raw_mode, enable_raw_mode},
};
use forge_sdk::KrakenClient;
use forge_sdk::model::message::KrakenMessage;
use ratatui::{prelude::*, widgets::*};
use std::{
    collections::HashMap,
    io,
    sync::{Arc, Mutex},
    time::{Duration, Instant},
};

// --- CONSTANTS ---
const TRADING_PAIRS: &[&str] = &["BTC/USD", "ETH/USD", "SOL/USD"];
const UI_POLL_MS: u64 = 100;

// --- CANDLE DATA ---
#[derive(Clone, Debug)]
struct CandleData {
    timestamp: f64,
    open: f64,
    high: f64,
    low: f64,
    close: f64,
}

// --- APP STATE ---
struct AppState {
    candles: HashMap<String, Vec<CandleData>>,
    logs: Vec<String>,
    curr_index: usize,
    pairs: Vec<String>,
    start_time: Instant,
}

impl AppState {
    fn new() -> Self {
        let mut candles = HashMap::new();
        let pairs: Vec<String> = TRADING_PAIRS.iter().map(|s| s.to_string()).collect();
        for p in &pairs {
            candles.insert(p.clone(), Vec::new());
        }
        Self {
            candles,
            logs: vec!["Initializing Chart Viewer...".to_string()],
            curr_index: 0,
            pairs,
            start_time: Instant::now(),
        }
    }

    fn log(&mut self, msg: String) {
        self.logs.push(msg);
        if self.logs.len() > 50 {
            self.logs.remove(0);
        }
    }
}

#[tokio::main]
async fn main() -> io::Result<()> {
    // Initialize TLS crypto provider
    rustls::crypto::ring::default_provider()
        .install_default()
        .ok();

    enable_raw_mode()?;
    let mut stdout = io::stdout();
    execute!(stdout, EnterAlternateScreen, EnableMouseCapture)?;
    let backend = CrosstermBackend::new(stdout);
    let mut terminal = Terminal::new(backend)?;

    let app_state = Arc::new(Mutex::new(AppState::new()));

    // Start network loop
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

// --- NETWORK LOOP ---
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

    // Subscribe to OHLC for all pairs
    for pair in TRADING_PAIRS {
        let sub = format!(
            r#"{{"method":"subscribe", "params":{{"channel":"ohlc", "symbol":["{}"], "interval":1}}}}"#,
            pair
        );
        let _ = client.send_raw(sub).await;
    }

    state
        .lock()
        .unwrap()
        .log("Connected & Subscribed to OHLC.".to_string());

    let mut stream = client.stream();
    while let Some(msg_res) = stream.recv().await {
        if let Ok(txt) = msg_res {
            // Log raw messages containing "ohlc" for debugging
            if txt.contains("\"channel\":\"ohlc\"") {
                state
                    .lock()
                    .unwrap()
                    .log(format!("📊 OHLC update received"));
            }

            if let Ok(msg) = serde_json::from_str::<KrakenMessage>(&txt) {
                if let KrakenMessage::OHLC(ohlc_msg) = msg {
                    let mut s = state.lock().unwrap();
                    for candle in ohlc_msg.data {
                        let symbol = candle.symbol.clone();
                        let timestamp = std::time::SystemTime::now()
                            .duration_since(std::time::UNIX_EPOCH)
                            .unwrap()
                            .as_secs() as f64;

                        s.log(format!("📈 Candle: {} ${:.2}", symbol, candle.close));

                        if let Some(candles) = s.candles.get_mut(&symbol) {
                            candles.push(CandleData {
                                timestamp,
                                open: candle.open,
                                high: candle.high,
                                low: candle.low,
                                close: candle.close,
                            });

                            // Keep last 100 candles
                            if candles.len() > 100 {
                                candles.remove(0);
                            }
                        }
                    }
                }
            }
        }
    }
}

// --- UI LOOP ---
fn run_ui_loop(
    terminal: &mut Terminal<CrosstermBackend<io::Stdout>>,
    state: Arc<Mutex<AppState>>,
) -> io::Result<()> {
    loop {
        terminal.draw(|f| ui(f, &state))?;

        if event::poll(Duration::from_millis(UI_POLL_MS))? {
            if let Event::Key(key) = event::read()? {
                match key.code {
                    KeyCode::Tab => {
                        let mut s = state.lock().unwrap();
                        s.curr_index = (s.curr_index + 1) % s.pairs.len();
                    }
                    KeyCode::Char('q') => return Ok(()),
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
            Constraint::Length(3), // Header
            Constraint::Min(10),   // Chart
            Constraint::Length(8), // Logs
            Constraint::Length(3), // Footer
        ])
        .split(f.area());

    let current_pair = &s.pairs[s.curr_index];

    render_header(f, chunks[0], current_pair);
    render_chart(f, chunks[1], &s, current_pair);
    render_logs(f, chunks[2], &s);
    render_footer(f, chunks[3], &s);
}

fn render_header(f: &mut Frame, area: Rect, pair: &str) {
    let title = format!("📈 KRAKEN CANDLESTICK CHART [{}] 📈", pair);
    let header = Paragraph::new(title)
        .style(
            Style::default()
                .fg(Color::Cyan)
                .add_modifier(Modifier::BOLD),
        )
        .alignment(Alignment::Center)
        .block(Block::default().borders(Borders::ALL));
    f.render_widget(header, area);
}

fn render_chart(f: &mut Frame, area: Rect, state: &AppState, pair: &str) {
    let data = state.candles.get(pair).map(|v| v.as_slice()).unwrap_or(&[]);

    if data.is_empty() {
        let placeholder = Paragraph::new("Waiting for OHLC data... (candles update every minute)")
            .style(Style::default().fg(Color::DarkGray))
            .alignment(Alignment::Center)
            .block(
                Block::default()
                    .title(" Candlestick Chart ")
                    .borders(Borders::ALL),
            );
        f.render_widget(placeholder, area);
        return;
    }

    // Calculate price bounds
    let min_price = data.iter().map(|c| c.low).fold(f64::INFINITY, f64::min);
    let max_price = data
        .iter()
        .map(|c| c.high)
        .fold(f64::NEG_INFINITY, f64::max);

    let price_range = max_price - min_price;
    if price_range == 0.0 {
        let placeholder = Paragraph::new("Insufficient price movement...")
            .style(Style::default().fg(Color::DarkGray))
            .alignment(Alignment::Center)
            .block(
                Block::default()
                    .title(" Candlestick Chart ")
                    .borders(Borders::ALL),
            );
        f.render_widget(placeholder, area);
        return;
    }

    // Build chart with proper axes
    let inner_area = Block::default()
        .title(format!(" {} - 1 Minute Candles ", pair))
        .borders(Borders::ALL)
        .inner(area);

    let chart_height = (inner_area.height.saturating_sub(2)) as usize;
    let chart_width = inner_area.width.saturating_sub(12) as usize; // Leave room for price labels

    if chart_height == 0 || chart_width == 0 {
        let block = Block::default()
            .title(" Candlestick Chart ")
            .borders(Borders::ALL);
        f.render_widget(block, area);
        return;
    }

    // Sample candles to fit width
    let num_candles = (chart_width / 2).min(data.len()); // 2 chars per candle (candle + space)
    let candles_to_show: Vec<&CandleData> = data.iter().rev().take(num_candles).rev().collect();

    // Build rows
    let mut lines: Vec<Line> = Vec::new();

    // Price labels
    let price_label_top = format!("${:.0}", max_price);
    let price_label_mid = format!("${:.0}", (max_price + min_price) / 2.0);
    let price_label_bot = format!("${:.0}", min_price);

    for row in 0..chart_height {
        let price_at_row = max_price - (row as f64 / chart_height as f64) * price_range;

        let mut spans = Vec::new();

        // Y-axis price label
        if row == 0 {
            spans.push(Span::styled(
                format!("{:>10} │", price_label_top),
                Style::default().fg(Color::Gray),
            ));
        } else if row == chart_height / 2 {
            spans.push(Span::styled(
                format!("{:>10} │", price_label_mid),
                Style::default().fg(Color::Gray),
            ));
        } else if row == chart_height - 1 {
            spans.push(Span::styled(
                format!("{:>10} │", price_label_bot),
                Style::default().fg(Color::Gray),
            ));
        } else {
            spans.push(Span::styled(
                "           │",
                Style::default().fg(Color::DarkGray),
            ));
        }

        // Draw candles
        for candle in &candles_to_show {
            let (char_to_draw, color) = get_candle_char(candle, price_at_row, price_range);
            spans.push(Span::styled(
                char_to_draw.to_string(),
                Style::default().fg(color),
            ));
            spans.push(Span::raw(" ")); // Space between candles
        }

        lines.push(Line::from(spans));
    }

    // X-axis label row
    let mut x_axis_spans = vec![Span::styled(
        "      Time │",
        Style::default().fg(Color::Gray),
    )];
    for _ in 0..num_candles.min(candles_to_show.len()) {
        x_axis_spans.push(Span::styled("──", Style::default().fg(Color::DarkGray)));
    }
    lines.push(Line::from(x_axis_spans));

    let paragraph = Paragraph::new(lines).block(
        Block::default()
            .title(format!(" {} - 1 Minute Candles ", pair))
            .borders(Borders::ALL),
    );

    f.render_widget(paragraph, area);
}

fn get_candle_char(candle: &CandleData, price_at_row: f64, price_range: f64) -> (char, Color) {
    let is_bullish = candle.close >= candle.open;
    let color = if is_bullish { Color::Green } else { Color::Red };

    let body_top = candle.close.max(candle.open);
    let body_bottom = candle.close.min(candle.open);

    let tolerance = price_range * 0.02;

    if price_at_row <= candle.high + tolerance && price_at_row >= candle.low - tolerance {
        if price_at_row <= body_top + tolerance && price_at_row >= body_bottom - tolerance {
            ('█', color)
        } else {
            ('│', color)
        }
    } else {
        (' ', Color::Reset)
    }
}

fn render_logs(f: &mut Frame, area: Rect, state: &AppState) {
    let log_items: Vec<ListItem> = state
        .logs
        .iter()
        .rev()
        .take(5)
        .map(|msg| ListItem::new(msg.clone()).style(Style::default().fg(Color::DarkGray)))
        .collect();

    let logs_list =
        List::new(log_items).block(Block::default().title(" Event Log ").borders(Borders::ALL));
    f.render_widget(logs_list, area);
}

fn render_footer(f: &mut Frame, area: Rect, state: &AppState) {
    let uptime = state.start_time.elapsed().as_secs();
    let current_pair = &state.pairs[state.curr_index];
    let candle_count = state
        .candles
        .get(current_pair)
        .map(|v| v.len())
        .unwrap_or(0);

    let footer_text = format!(
        " [Tab] Switch Pair | [Q] Quit | {} | Candles: {} | Uptime: {}s ",
        current_pair, candle_count, uptime
    );

    let footer = Paragraph::new(footer_text)
        .style(Style::default().fg(Color::Cyan))
        .alignment(Alignment::Center)
        .block(Block::default().borders(Borders::ALL));
    f.render_widget(footer, area);
}
