use crate::app::App;
use ratatui::{
    layout::{Alignment, Constraint, Direction, Layout},
    style::{Color, Modifier, Style},
    text::{Line, Span},
    widgets::{Block, Borders, Paragraph},
    Frame,
};

/// Minimum column width (chars) that keeps all labels unclipped.
/// "Fundamentals  55.0" = 18 content + 2×2 margin + 2 border = 24.
const MIN_COL_WIDTH: u16 = 24;

pub fn render(f: &mut Frame, app: &App) {
    let size = f.area();
    if app.results.is_empty() {
        return;
    }

    let chunks = Layout::default()
        .direction(Direction::Vertical)
        .constraints([
            Constraint::Length(3), // Header
            Constraint::Min(0),    // Columns
            Constraint::Length(1), // Footer
        ])
        .split(size);

    // Header
    let title_block = Block::default()
        .borders(Borders::ALL)
        .border_style(Style::default().fg(Color::Cyan));
    let title_para = Paragraph::new(Span::styled(
        " VORA-VISION — Comparison Matrix ",
        Style::default().fg(Color::Cyan).add_modifier(Modifier::BOLD),
    ))
    .block(title_block)
    .alignment(Alignment::Center);
    f.render_widget(title_para, chunks[0]);

    let n = app.results.len();
    let available_w = chunks[1].width;

    // How many columns fit at the minimum width?
    let max_visible = ((available_w / MIN_COL_WIDTH) as usize).max(1).min(n);

    // Sliding window: keep the focused column visible.
    let focused = app.compare_cursor.min(n - 1);
    let start = if focused + 1 >= max_visible {
        (focused + 1 - max_visible).min(n - max_visible)
    } else {
        0
    };
    let end = (start + max_visible).min(n);
    let visible = &app.results[start..end];

    // Equal-width columns for the visible slice.
    let constraints: Vec<Constraint> = (0..visible.len())
        .map(|_| Constraint::Ratio(1, visible.len() as u32))
        .collect();
    let columns = Layout::default()
        .direction(Direction::Horizontal)
        .constraints(constraints)
        .split(chunks[1]);

    let highest_score = app.results.iter()
        .map(|r| r.composite_score)
        .fold(0.0f64, |a, b| a.max(b));

    for (slot, result) in visible.iter().enumerate() {
        let global_idx = start + slot;
        let is_focused = global_idx == focused;
        let is_leader = (result.composite_score - highest_score).abs() < 0.0001;

        let border_color = if is_focused {
            Color::Yellow
        } else if is_leader {
            Color::Green
        } else {
            Color::DarkGray
        };

        // Build title; append scroll hint on the last visible slot when clipped.
        let mut title = if is_leader {
            format!(" {} (LEADER) ", result.ticker)
        } else {
            format!(" {} ", result.ticker)
        };
        if slot == visible.len() - 1 && end < n {
            title.push_str(&format!("  [{}/{}] → ", end, n));
        } else if slot == 0 && start > 0 {
            title = format!(" ← [{}/{}] {}", start + 1, n, title.trim_start());
        }

        let block = Block::default()
            .borders(Borders::ALL)
            .title(title)
            .border_style(Style::default().fg(border_color).add_modifier(
                if is_focused { Modifier::BOLD } else { Modifier::empty() }
            ));

        let rect = columns[slot];
        f.render_widget(block, rect);

        let inner = rect.inner(ratatui::layout::Margin { horizontal: 2, vertical: 2 });

        let col_chunks = Layout::default()
            .direction(Direction::Vertical)
            .constraints([
                Constraint::Length(1), // top spacing
                Constraint::Length(3), // signal badge
                Constraint::Length(3), // score / coverage
                Constraint::Length(1), // separator
                Constraint::Min(0),    // category rows
            ])
            .split(inner);

        // Signal badge
        let sig_color = result.signal.color();
        f.render_widget(
            Paragraph::new(Span::styled(
                format!("  {}  ", result.signal),
                Style::default().fg(Color::Black).bg(sig_color).add_modifier(Modifier::BOLD),
            )).alignment(Alignment::Center),
            col_chunks[1],
        );

        // Score + coverage
        f.render_widget(
            Paragraph::new(vec![
                Line::from(Span::styled(
                    format!("{:.1}", result.composite_score),
                    Style::default().fg(Color::White).add_modifier(Modifier::BOLD),
                )),
                Line::from("Composite Score"),
                Line::from(Span::styled(
                    format!("Coverage: {:.0}%", result.confidence_score),
                    Style::default().fg(Color::DarkGray),
                )),
            ]).alignment(Alignment::Center),
            col_chunks[2],
        );

        // Separator
        f.render_widget(
            Paragraph::new("─────────────────────")
                .style(Style::default().fg(Color::DarkGray))
                .alignment(Alignment::Center),
            col_chunks[3],
        );

        // Category rows — name left-padded to 12, score right-aligned to 5
        let cat = |name: &str, score: f64| {
            let color = if score >= 65.0 { Color::Green }
                        else if score >= 41.0 { Color::Yellow }
                        else { Color::Red };
            Line::from(vec![
                Span::styled(format!("{:<12}", name), Style::default().fg(Color::Gray)),
                Span::styled(format!("{:>5.1}", score), Style::default().fg(color).add_modifier(Modifier::BOLD)),
            ])
        };

        let cat_lines = vec![
            cat("Valuation", result.valuation.raw_score),
            Line::from(""),
            cat("Fundamentals", result.fundamentals.raw_score),
            Line::from(""),
            cat("Macro Env", result.macro_env.raw_score),
            Line::from(""),
            cat("Sentiment", result.sentiment.raw_score),
            Line::from(""),
            cat("Risk Profile", result.risk.raw_score),
        ];
        f.render_widget(
            Paragraph::new(cat_lines).alignment(Alignment::Left),
            col_chunks[4],
        );
    }

    // Footer
    let scroll_hint = if n > max_visible {
        format!("  [{}/{}]  |  ", focused + 1, n)
    } else {
        "  ".to_string()
    };
    let footer_text = Line::from(vec![
        Span::styled(" ←/→ ", Style::default().fg(Color::Cyan).add_modifier(Modifier::BOLD)),
        Span::raw(format!("Select Ticker{}|  ", scroll_hint)),
        Span::styled(" Enter ", Style::default().fg(Color::Green).add_modifier(Modifier::BOLD)),
        Span::raw("View Report  |  "),
        Span::styled(" B ", Style::default().fg(Color::Green).add_modifier(Modifier::BOLD)),
        Span::raw("Backtest  |  "),
        Span::styled(" K ", Style::default().fg(Color::Green).add_modifier(Modifier::BOLD)),
        Span::raw("Calibrate  |  "),
        Span::styled(" R ", Style::default().fg(Color::Red).add_modifier(Modifier::BOLD)),
        Span::raw("Reset  |  "),
        Span::styled(" Q ", Style::default().fg(Color::Red).add_modifier(Modifier::BOLD)),
        Span::raw("Quit"),
    ]);
    f.render_widget(
        Paragraph::new(footer_text)
            .alignment(Alignment::Center)
            .style(Style::default().fg(Color::Black).bg(Color::DarkGray)),
        chunks[2],
    );
}
