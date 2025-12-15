use std::{io, sync::{Arc, Mutex}, time::Duration};
use crossterm::{
    event::{self, DisableMouseCapture, EnableMouseCapture, Event, KeyCode},
    execute,
    terminal::{disable_raw_mode, enable_raw_mode, EnterAlternateScreen, LeaveAlternateScreen},
};
use ratatui::{prelude::*, widgets::*};
use tokio::sync::mpsc;

// Import your SDK
use forge_sdk::KrakenClient;
use forge_sdk::model::message::KrakenMessage;
use forge_sdk::book::orderbook::LocalBook;
use forge_sdk::book::checksum::validate_checksum;

// --- STATE MANAGEMENT ---
struct AppState {
    pub book: LocalBook,
    pub logs: Vec<String>,
    pub checksum_valid: bool,
    pub last_checksum: u32,
}

impl AppState {
    fn new() -> Self {
        Self {
            book: LocalBook::new(),
            logs: vec!["Initializing Kraken Forge...".to_string()],
            checksum_valid: true,
            last_checksum: 0,
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
    rustls::crypto::ring::default_provider().install_default().ok();

    // 2. Setup Terminal
    enable_raw_mode()?;
    let mut stdout = io::stdout();
    execute!(stdout, EnterAlternateScreen, EnableMouseCapture)?;
    let backend = CrosstermBackend::new(stdout);
    let mut terminal = Terminal::new(backend)?;

    // 3. Shared State
    let app_state = Arc::new(Mutex::new(AppState::new()));

    // 4. SPAWN BACKGROUND NETWORK TASK
    let state_clone = app_state.clone();
    tokio::spawn(async move {
        run_network_loop(state_clone).await;
    });

    // 5. RUN UI LOOP (Main Thread)
    let res = run_ui_loop(&mut terminal, app_state);

    // 6. Cleanup
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

// --- THE NETWORK ENGINE ---
async fn run_network_loop(state: Arc<Mutex<AppState>>) {
    let mut client = match KrakenClient::connect("wss://ws.kraken.com/v2").await {
        Ok(c) => c,
        Err(e) => {
            state.lock().unwrap().log(format!("Connection Error: {}", e));
            return;
        }
    };

    // Subscribe to Book and Trade
    let _ = client.subscribe_book(&["BTC/USD"], 10).await;
    
    // Manual Trade Sub command
    let trade_cmd = r#"{"method":"subscribe", "params":{"channel":"trade", "symbol":["BTC/USD"]}}"#.to_string();
    let _ = client.send_command(trade_cmd).await;

    let mut stream = client.stream();
    state.lock().unwrap().log("✅ Connected & Subscribed.".to_string());

    while let Some(msg_res) = stream.recv().await {
        if let Ok(txt) = msg_res {
            // NOTE: Using serde_json directly here. If you kept the regex hack, insert it here.
            if let Ok(parsed) = serde_json::from_str::<KrakenMessage>(&txt) {
                let mut s = state.lock().unwrap();
                
                match parsed {
                    KrakenMessage::Book(msg) => {
                        let data = &msg.data[0];
                        if msg.r#type == "snapshot" {
                            s.book.apply_snapshot(data.bids.clone(), data.asks.clone());
                            s.log("📸 Snapshot Loaded".to_string());
                        } else {
                            s.book.apply_updates(data.bids.clone(), true);
                            s.book.apply_updates(data.asks.clone(), false);
                        }

                        // Checksum Logic
                        s.checksum_valid = validate_checksum(&s.book, data.checksum);
                        s.last_checksum = data.checksum;
                        if !s.checksum_valid {
                            s.log(format!("❌ Checksum Mismatch! RPC: {}", data.checksum));
                        }
                    }
                    KrakenMessage::Trade(msg) => {
                        for t in msg.data {
                            let val = t.price * t.qty;
                            // Whale Alert Logic > $50k
                            if val > 50_000.0 {
                                s.log(format!("🚨 WHALE ALERT: {} sold {:.4} BTC (${:.0})", 
                                    t.side.to_uppercase(), t.qty, val));
                            } else if val > 10_000.0 {
                                // Log medium trades
                                s.log(format!("💰 Trade: {} ${:.0}", t.side, t.price));
                            }
                        }
                    }
                    KrakenMessage::Heartbeat { .. } => {} // Ignore
                    _ => {} 
                }
            }
        }
    }
}

// --- THE UI RENDER LOOP ---
fn run_ui_loop(
    terminal: &mut Terminal<CrosstermBackend<io::Stdout>>,
    state: Arc<Mutex<AppState>>,
) -> io::Result<()> {
    loop {
        // Draw
        terminal.draw(|f| ui(f, &state))?;

        // Handle Input (Poll fast for responsiveness)
        if event::poll(Duration::from_millis(50))? {
            if let Event::Key(key) = event::read()? {
                if let KeyCode::Char('q') = key.code {
                    return Ok(());
                }
            }
        }
    }
}

// --- DRAWING THE WIDGETS ---
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
        .split(f.size());

    // 2. Main Content Split: Book (Left), Logs (Right)
    let main_chunks = Layout::default()
        .direction(Direction::Horizontal)
        .constraints([Constraint::Percentage(50), Constraint::Percentage(50)])
        .split(chunks[1]);

    // --- WIDGET 1: HEADER ---
    let title = Paragraph::new("🐙 KRAKEN FORGE SDK TERMINAL 🐙")
        .style(Style::default().fg(Color::Cyan).add_modifier(Modifier::BOLD))
        .alignment(Alignment::Center)
        .block(Block::default().borders(Borders::ALL));
    f.render_widget(title, chunks[0]);

    // --- WIDGET 2: ORDER BOOK (LEFT) ---
    // Prepare rows
    let mut rows = Vec::new();
    
    // Header Row
    rows.push(Row::new(vec!["Side", "Price", "Qty"]).style(Style::default().add_modifier(Modifier::UNDERLINED)));

    // Asks (Red - Sell Side) - Top 10 Reversed (Lowest Asks first)
    for (price, (p_str, q_str)) in s.book.asks.iter().take(10) {
        rows.push(Row::new(vec![
            Cell::from("ASK").style(Style::default().fg(Color::Red)),
            Cell::from(p_str.as_str()),
            Cell::from(q_str.as_str()),
        ]));
    }

    // Divider
    rows.push(Row::new(vec!["---", "---", "---"]));

    // Bids (Green - Buy Side) - Top 10 Reversed (Highest Bids first)
    for (price, (p_str, q_str)) in s.book.bids.iter().rev().take(10) {
        rows.push(Row::new(vec![
            Cell::from("BID").style(Style::default().fg(Color::Green)),
            Cell::from(p_str.as_str()),
            Cell::from(q_str.as_str()),
        ]));
    }

    let book_table = Table::new(rows, [Constraint::Percentage(20), Constraint::Percentage(40), Constraint::Percentage(40)])
        .block(Block::default().title(" Order Book (BTC/USD) ").borders(Borders::ALL));
    f.render_widget(book_table, main_chunks[0]);


    // --- WIDGET 3: LOGS / WHALE WATCH (RIGHT) ---
    let log_items: Vec<ListItem> = s.logs
        .iter()
        .rev() // Show newest at top
        .take(20)
        .map(|msg| {
            let style = if msg.contains("WHALE") {
                Style::default().fg(Color::Yellow).add_modifier(Modifier::BOLD | Modifier::RAPID_BLINK)
            } else if msg.contains("Mismatch") {
                Style::default().fg(Color::Red).bg(Color::White)
            } else {
                Style::default().fg(Color::White)
            };
            ListItem::new(msg.clone()).style(style)
        })
        .collect();

    let logs_list = List::new(log_items)
        .block(Block::default().title(" Live Event Feed ").borders(Borders::ALL));
    f.render_widget(logs_list, main_chunks[1]);


    // --- WIDGET 4: FOOTER (CHECKSUM STATUS) ---
    let status_text = if s.checksum_valid {
        format!("Checksum Status: ✅ VALID | Last RPC: {}", s.last_checksum)
    } else {
        format!("Checksum Status: ❌ INVALID | Last RPC: {}", s.last_checksum)
    };
    
    let status_style = if s.checksum_valid { 
        Style::default().fg(Color::Green) 
    } else { 
        Style::default().fg(Color::Red).add_modifier(Modifier::BOLD) 
    };

    let footer = Paragraph::new(status_text)
        .style(status_style)
        .block(Block::default().borders(Borders::ALL));
    f.render_widget(footer, chunks[2]);
}