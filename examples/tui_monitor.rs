use std::{fmt::format, io, sync::{Arc, Mutex}, time::Duration};
use crossterm::{event::{self, DisableMouseCapture, EnableMouseCapture, Event, KeyCode},
    execute,
    terminal::{disable_raw_mode, enable_raw_mode, EnterAlternateScreen, LeaveAlternateScreen}
};
use ratatui::{prelude::*, widgets::*};
use tokio::sync::mpsc;

use forge_sdk::KrakenClient;
use forge_sdk::model::message::{KrakenMessage, BookMessage, BookData, BookLevel};
use forge_sdk::book::orderbook::LocalBook;
use forge_sdk::book::checksum::validate_checksum;
use regex::Regex;

fn manually_parse_book_message(json: &str) -> Result<KrakenMessage, serde_json::Error> {
    let channel_re = Regex::new(r#""channel":"([^"]+)""#).unwrap();
    let type_re = Regex::new(r#""type":"([^"]+)""#).unwrap();
    let symbol_re = Regex::new(r#""symbol":"([^"]+)""#).unwrap();
    let checksum_re = Regex::new(r#""checksum":(\d+)"#).unwrap();
    
    let channel = channel_re.captures(json).and_then(|c| c.get(1)).map(|m| m.as_str().to_string()).unwrap_or_default();
    let type_str = type_re.captures(json).and_then(|c| c.get(1)).map(|m| m.as_str().to_string()).unwrap_or_default();
    let symbol = symbol_re.captures(json).and_then(|c| c.get(1)).map(|m| m.as_str().to_string()).unwrap_or_default();
    let checksum: u32 = checksum_re.captures(json).and_then(|c| c.get(1)).and_then(|m| m.as_str().parse().ok()).unwrap_or(0);
    
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
    
    let array_content = array_re.captures(json)
        .and_then(|c| c.get(1))
        .map(|m| m.as_str())
        .unwrap_or("");
    
    let obj_re = Regex::new(r#"\{"price":([\d.]+),"qty":([\d.]+)\}"#).unwrap();
    
    obj_re.captures_iter(array_content)
        .map(|cap| {
            let price = cap.get(1).unwrap().as_str().to_string();
            let qty = cap.get(2).unwrap().as_str().to_string();
            BookLevel { price, qty }
        })
        .collect()
}

struct AppState {
    pub book: LocalBook,
    pub logs: Vec<String>,
    pub checksum_valid: bool,
    pub last_checksum: u32,
}

impl AppState{
    fn new() -> Self {
        Self {
            book: LocalBook::new(),
            logs: vec!["Initializing Kraken ...".to_string()],
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

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    // 1. Setup Crypto Provider
    rustls::crypto::ring::default_provider().install_default().ok();

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

async fn run_network_loop(state: Arc<Mutex<AppState>>) {
    let mut client = match KrakenClient::connect("wss://ws.kraken.com/v2").await {
        Ok(c) => c,
        Err(e) => {
            state.lock().unwrap().log(format!("Connection Error: {}" , e));
            return ;
        }
    };

    let _ = client.subscribe_book(&["BTC/USD"], 10).await;

    let trade_cmd = r#"{"method":"subscribe", "params":{"channel":"trade", "symbol":["BTC/USD"]}}"#.to_string();
    let _ = client.send_command(trade_cmd).await;

    let mut stream = client.stream();
    state.lock().unwrap().log("Connected & Subscribed.".to_string());

    while let Some(msg_res)  = stream.recv().await {
        if let Ok(txt) = msg_res {
            let parsed = if txt.contains("\"channel\":\"book\"") {
                manually_parse_book_message(&txt)
            } else {
                serde_json::from_str::<KrakenMessage>(&txt)
            };
            
            if let Ok(parsed) = parsed {
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
                        KrakenMessage::Heartbeat { .. } => {}
                        _=> {}
                    }
            }
        }
    }
}

fn run_ui_loop(
    terminal: &mut Terminal<CrosstermBackend<io::Stdout>>,
    state: Arc<Mutex<AppState>>,
) -> io::Result<()> {
    loop {
        terminal.draw(|f| ui(f, &state))?;

        if event::poll(Duration::from_millis(50))? {
            if let Event::Key(key) = event::read()? {
                if let KeyCode::Char('q') = key.code {
                    return Ok(());
                }
            }
        }
    }
}

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