use crate::app::App;
use crate::sim::TradeAction;
use ratatui::{
    layout::{Alignment, Constraint, Direction, Layout, Rect},
    style::{Color, Modifier, Style},
    text::{Line, Span},
    widgets::{Block, Borders, List, ListItem, Paragraph, Row, Table},
    Frame,
};

pub fn render(f: &mut Frame, app: &App) {
    let size = f.area();

    let chunks = Layout::default()
        .direction(Direction::Vertical)
        .constraints([
            Constraint::Length(4), // summary
            Constraint::Min(6),    // holdings
            Constraint::Length(7), // recent trades
            Constraint::Length(1), // footer
        ])
        .split(size);

    render_summary(f, chunks[0], app);
    render_holdings(f, chunks[1], app);
    render_trades(f, chunks[2], app);

    let footer = Line::from(vec![
        Span::styled(" Esc/Enter ", Style::default().fg(Color::Cyan).add_modifier(Modifier::BOLD)),
        Span::raw("Back  "),
        Span::styled(" R ", Style::default().fg(Color::Red).add_modifier(Modifier::BOLD)),
        Span::raw("New Search  "),
        Span::styled(" Q ", Style::default().fg(Color::Red).add_modifier(Modifier::BOLD)),
        Span::raw("Quit"),
    ]);
    f.render_widget(
        Paragraph::new(footer)
            .alignment(Alignment::Center)
            .style(Style::default().fg(Color::Black).bg(Color::DarkGray)),
        chunks[3],
    );
}

fn render_summary(f: &mut Frame, rect: Rect, app: &App) {
    let pp = &app.paper;
    let total = pp.total_value();
    let ret = pp.total_return_pct();
    let ret_color = if ret >= 0.0 { Color::Green } else { Color::Red };

    let mut lines = vec![Line::from(vec![
        Span::styled(
            format!("Value: ${:.2}  ", total),
            Style::default().fg(Color::White).add_modifier(Modifier::BOLD),
        ),
        Span::styled(format!("({:+.2}%)  ", ret), Style::default().fg(ret_color).add_modifier(Modifier::BOLD)),
        Span::styled(format!("Cash: ${:.2}  ", pp.portfolio.cash), Style::default().fg(Color::Gray)),
        Span::styled(
            format!("Since {}", pp.created),
            Style::default().fg(Color::DarkGray),
        ),
    ])];
    if let Some(msg) = &app.status_message {
        lines.push(Line::from(Span::styled(msg.clone(), Style::default().fg(Color::Yellow))));
    }

    f.render_widget(
        Paragraph::new(lines)
            .block(
                Block::default()
                    .borders(Borders::ALL)
                    .title(" Paper Portfolio (follows live signals) ")
                    .border_style(Style::default().fg(Color::Cyan)),
            )
            .alignment(Alignment::Left),
        rect,
    );
}

fn render_holdings(f: &mut Frame, rect: Rect, app: &App) {
    let pp = &app.paper;
    let hdr = Style::default().fg(Color::Cyan).add_modifier(Modifier::BOLD);

    if pp.portfolio.positions.is_empty() {
        let p = Paragraph::new("No open positions yet. From a result screen, press P to invest per the signal.")
            .block(Block::default().borders(Borders::ALL).title(" Holdings ").border_style(Style::default().fg(Color::DarkGray)))
            .style(Style::default().fg(Color::Gray))
            .alignment(Alignment::Center);
        f.render_widget(p, rect);
        return;
    }

    let mut tickers: Vec<&String> = pp.portfolio.positions.keys().collect();
    tickers.sort();

    let rows: Vec<Row> = tickers
        .iter()
        .filter_map(|t| {
            let pos = pp.portfolio.positions.get(*t)?;
            let last = pp.last_prices.get(*t).copied().unwrap_or(pos.avg_cost);
            let mkt = pos.shares * last;
            let pl_pct = if pos.avg_cost > 0.0 {
                (last / pos.avg_cost - 1.0) * 100.0
            } else {
                0.0
            };
            Some(Row::new(vec![
                (*t).clone(),
                format!("{:.2}", pos.shares),
                format!("${:.2}", pos.avg_cost),
                format!("${:.2}", last),
                format!("${:.2}", mkt),
                format!("{:+.1}%", pl_pct),
            ]))
        })
        .collect();

    let table = Table::new(
        rows,
        [
            Constraint::Percentage(18),
            Constraint::Percentage(16),
            Constraint::Percentage(16),
            Constraint::Percentage(16),
            Constraint::Percentage(18),
            Constraint::Percentage(16),
        ],
    )
    .header(Row::new(vec!["Ticker", "Shares", "Avg Cost", "Last", "Mkt Value", "P/L"]).style(hdr))
    .block(Block::default().borders(Borders::ALL).title(" Holdings ").border_style(Style::default().fg(Color::DarkGray)));
    f.render_widget(table, rect);
}

fn render_trades(f: &mut Frame, rect: Rect, app: &App) {
    let pp = &app.paper;
    let trades = &pp.portfolio.trades;

    let items: Vec<ListItem> = if trades.is_empty() {
        vec![ListItem::new("No trades recorded yet.")]
    } else {
        trades
            .iter()
            .rev()
            .take(rect.height.saturating_sub(2) as usize)
            .map(|t| {
                let (label, color) = match t.action {
                    TradeAction::Buy => ("BUY ", Color::Green),
                    TradeAction::Sell => ("SELL", Color::Red),
                };
                ListItem::new(Line::from(vec![
                    Span::styled(format!("{} ", t.date), Style::default().fg(Color::DarkGray)),
                    Span::styled(format!("{} ", label), Style::default().fg(color).add_modifier(Modifier::BOLD)),
                    Span::styled(format!("{:<6}", t.ticker), Style::default().fg(Color::White)),
                    Span::raw(format!("{:.2} @ ${:.2}  ", t.shares, t.price)),
                    Span::styled(format!("(score {:.1})", t.score), Style::default().fg(Color::DarkGray)),
                ]))
            })
            .collect()
    };

    let list = List::new(items)
        .block(Block::default().borders(Borders::ALL).title(" Recent Trades ").border_style(Style::default().fg(Color::DarkGray)))
        .style(Style::default().fg(Color::Gray));
    f.render_widget(list, rect);
}
