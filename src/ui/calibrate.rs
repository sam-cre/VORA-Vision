use crate::app::App;
use ratatui::{
    layout::{Alignment, Constraint, Direction, Layout, Rect},
    style::{Color, Modifier, Style},
    text::{Line, Span},
    widgets::{Block, Borders, Paragraph, Row, Table},
    Frame,
};

pub fn render(f: &mut Frame, app: &App) {
    let size = f.area();
    let cal = match &app.calibration {
        Some(c) => c,
        None => {
            let p = Paragraph::new("No calibration data. Press R to return to search.")
                .alignment(Alignment::Center)
                .style(Style::default().fg(Color::Red));
            f.render_widget(p, size);
            return;
        }
    };

    let chunks = Layout::default()
        .direction(Direction::Vertical)
        .constraints([
            Constraint::Length(3), // header
            Constraint::Min(8),    // table
            Constraint::Length(3), // current-score readout
            Constraint::Length(1), // footer
        ])
        .split(size);

    let header = format!(
        " VORA-VISION — Score Calibration · {}-month forward returns · {} observations ",
        cal.horizon_months, cal.n_total
    );
    f.render_widget(
        Paragraph::new(Span::styled(
            header,
            Style::default().fg(Color::Cyan).add_modifier(Modifier::BOLD),
        ))
        .block(Block::default().borders(Borders::ALL).border_style(Style::default().fg(Color::Cyan)))
        .alignment(Alignment::Center),
        chunks[0],
    );

    render_table(f, chunks[1], app);
    render_current(f, chunks[2], app);

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

fn render_table(f: &mut Frame, rect: Rect, app: &App) {
    let cal = app.calibration.as_ref().unwrap();
    let hdr = Style::default().fg(Color::Cyan).add_modifier(Modifier::BOLD);
    let current_bucket = app.calib_current_score.and_then(|s| cal.bucket_for(s)).map(|b| (b.lo, b.hi));

    let rows: Vec<Row> = cal
        .buckets
        .iter()
        .map(|b| {
            let band = format!("{:.0}-{:.0}", b.lo, b.hi);
            let (avg, median, win, spread, n) = if b.n == 0 {
                ("—".to_string(), "—".to_string(), "—".to_string(), "—".to_string(), "0".to_string())
            } else {
                (
                    format!("{:+.1}%", b.mean),
                    format!("{:+.1}%", b.median),
                    format!("{:.0}%", b.win_rate),
                    format!("{:+.0}% to {:+.0}%", b.p25, b.p75),
                    format!("{}", b.n),
                )
            };
            let mut style = if b.mean > 0.0 && b.n > 0 {
                Style::default().fg(Color::Green)
            } else if b.n > 0 {
                Style::default().fg(Color::Red)
            } else {
                Style::default().fg(Color::DarkGray)
            };
            // Highlight the band the current score falls into.
            if current_bucket == Some((b.lo, b.hi)) {
                style = style.add_modifier(Modifier::BOLD | Modifier::REVERSED);
            }
            Row::new(vec![band, n, avg, median, win, spread]).style(style)
        })
        .collect();

    let table = Table::new(
        rows,
        [
            Constraint::Percentage(15),
            Constraint::Percentage(8),
            Constraint::Percentage(15),
            Constraint::Percentage(15),
            Constraint::Percentage(13),
            Constraint::Percentage(34),
        ],
    )
    .header(Row::new(vec!["Score band", "N", "Avg fwd", "Median", "Win rate", "Likely range (p25–p75)"]).style(hdr))
    .block(
        Block::default()
            .borders(Borders::ALL)
            .title(" Forward return by score band ")
            .border_style(Style::default().fg(Color::DarkGray)),
    );
    f.render_widget(table, rect);
}

fn render_current(f: &mut Frame, rect: Rect, app: &App) {
    let cal = app.calibration.as_ref().unwrap();
    let line = match app.calib_current_score {
        Some(score) => match cal.bucket_for(score) {
            Some(b) => Line::from(vec![
                Span::styled(
                    format!("Current score {:.1} ", score),
                    Style::default().fg(Color::White).add_modifier(Modifier::BOLD),
                ),
                Span::raw(format!("(band {:.0}-{:.0}): historically ", b.lo, b.hi)),
                Span::styled(
                    format!("{:+.1}% ", b.mean),
                    Style::default()
                        .fg(if b.mean >= 0.0 { Color::Green } else { Color::Red })
                        .add_modifier(Modifier::BOLD),
                ),
                Span::raw(format!(
                    "avg over {} months  ·  middle 50% landed {:+.0}% to {:+.0}%  ·  {:.0}% positive  ·  n={}",
                    cal.horizon_months, b.p25, b.p75, b.win_rate, b.n
                )),
            ]),
            None => Line::from(Span::styled(
                format!("Current score {:.1}: no historical observations in this band.", score),
                Style::default().fg(Color::DarkGray),
            )),
        },
        None => Line::from(Span::styled(
            "Backtested expectation by score band. Not a prediction — past behavior only.",
            Style::default().fg(Color::DarkGray),
        )),
    };
    f.render_widget(
        Paragraph::new(line)
            .block(Block::default().borders(Borders::ALL).border_style(Style::default().fg(Color::DarkGray)))
            .alignment(Alignment::Center),
        rect,
    );
}
