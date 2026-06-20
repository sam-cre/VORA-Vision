use crate::app::App;
use ratatui::{
    layout::{Alignment, Constraint, Layout},
    style::{Color, Style},
    widgets::{Block, Borders, Paragraph, Wrap},
    Frame,
};

pub fn render(f: &mut Frame, _app: &App, err_msg: &str) {
    let chunks = Layout::default()
        .constraints([
            Constraint::Percentage(30),
            Constraint::Percentage(40),
            Constraint::Percentage(30),
        ])
        .split(f.area());
    let block = Block::default()
        .borders(Borders::ALL)
        .title("Error");
    let paragraph = Paragraph::new(format!(
        "An error occurred:\n\n{}\n\nPress any key to return to input.",
        err_msg
    ))
    .block(block)
    .alignment(Alignment::Center)
    .wrap(Wrap { trim: true })
    .style(Style::default().fg(Color::Red));
    f.render_widget(paragraph, chunks[1]);
}
