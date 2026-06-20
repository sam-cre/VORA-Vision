use crate::app::App;
use crate::models::{DataSource, FetchStatus};
use ratatui::{
    layout::{Alignment, Constraint, Direction, Layout},
    style::{Color, Modifier, Style},
    text::{Line, Span},
    widgets::{Block, Borders, Paragraph},
    Frame,
};

pub fn render(f: &mut Frame, app: &App) {
    let size = f.area();

    let chunks = Layout::default()
        .direction(Direction::Vertical)
        .constraints([
            Constraint::Length(3), // Title
            Constraint::Min(0),    // Center List
            Constraint::Length(1), // Bottom text
        ])
        .split(size);

    // Title
    let title_block = Block::default()
        .borders(Borders::ALL)
        .border_style(Style::default().fg(Color::Cyan));
    let title_text = if app.loading_label.is_empty() {
        "VORA-VISION — Fetching Financial Intelligence".to_string()
    } else {
        format!("VORA-VISION — {}", app.loading_label)
    };
    let title = Paragraph::new(Span::styled(
        title_text,
        Style::default().fg(Color::Cyan).add_modifier(Modifier::BOLD),
    ))
    .block(title_block)
    .alignment(Alignment::Center);
    f.render_widget(title, chunks[0]);

    // Spinner characters
    let spinner_chars = ["|", "/", "-", "\\"];
    let spinner_idx = (app.tick_count % spinner_chars.len() as u64) as usize;
    let spinner = spinner_chars[spinner_idx];

    // Build the lines for the 4 sources
    let mut list_lines = Vec::new();
    list_lines.push(Line::from("")); // padding

    for &(src, ref status) in &app.fetch_progress {
        let (icon, label_style) = match status {
            FetchStatus::Pending => (
                Span::styled(format!(" {} ", spinner), Style::default().fg(Color::Yellow).add_modifier(Modifier::BOLD)),
                Style::default().fg(Color::White),
            ),
            FetchStatus::Success => (
                Span::styled(" OK ", Style::default().fg(Color::Green).add_modifier(Modifier::BOLD)),
                Style::default().fg(Color::Gray),
            ),
            FetchStatus::Failed(_) => (
                Span::styled("FAIL", Style::default().fg(Color::Red).add_modifier(Modifier::BOLD)),
                Style::default().fg(Color::Red),
            ),
        };

        let src_name = match src {
            DataSource::Yahoo => "Yahoo Finance (Fundamentals & Price History)",
            DataSource::Fred => "FRED (Macroeconomic Indicators)",
            DataSource::Edgar => "SEC EDGAR (Annual Financial Statements)",
            DataSource::Finnhub => "Finnhub (Peers & Market Sentiment)",
        };

        let mut line_spans = vec![
            Span::raw("["),
            icon,
            Span::raw("] "),
            Span::styled(src_name, label_style),
        ];

        if let FetchStatus::Failed(err_msg) = status {
            line_spans.push(Span::styled(format!(" — Failed: {}", err_msg), Style::default().fg(Color::Red)));
        }

        list_lines.push(Line::from(line_spans));
        list_lines.push(Line::from("")); // space between items
    }

    let center_layout = Layout::default()
        .direction(Direction::Horizontal)
        .constraints([
            Constraint::Percentage(15),
            Constraint::Percentage(70),
            Constraint::Percentage(15),
        ])
        .split(chunks[1]);

    let list_block = Block::default()
        .borders(Borders::ALL)
        .title(" Progress ")
        .border_style(Style::default().fg(Color::DarkGray));
    
    let list_para = Paragraph::new(list_lines)
        .block(list_block)
        .alignment(Alignment::Left);
        
    f.render_widget(list_para, center_layout[1]);

    // Bottom
    let bottom = Paragraph::new("Analyzing market data, computing signals, please wait...")
        .alignment(Alignment::Center)
        .style(Style::default().fg(Color::DarkGray));
    f.render_widget(bottom, chunks[2]);
}
