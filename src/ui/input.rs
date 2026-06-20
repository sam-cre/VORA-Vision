use crate::app::{App, InputFocus};
use crate::models::Horizon;
use ratatui::{
    layout::{Alignment, Constraint, Direction, Layout, Rect},
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
            Constraint::Length(1),  // Spacing
            Constraint::Length(10), // Title (ASCII Art)
            Constraint::Length(2),  // Instruction
            Constraint::Length(3),  // Text Input Box
            Constraint::Length(1),  // Separator
            Constraint::Length(3),  // Horizon Selector
            Constraint::Length(1),  // Spacing
            Constraint::Length(11), // Horizon explanation
            Constraint::Min(0),     // Spacer pushing empty space below
            Constraint::Length(1),  // Hint bar
        ])
        .split(size);

    // Title (ASCII Art Logo)
    let logo_lines = vec![
        Line::from(Span::styled(" _    ______  ____  ___           ", Style::default().fg(Color::Cyan).add_modifier(Modifier::BOLD))),
        Line::from(Span::styled("| |  / / __ \\/ __ \\/   |          ", Style::default().fg(Color::Cyan).add_modifier(Modifier::BOLD))),
        Line::from(Span::styled("| | / / / / / /_/ / /| |          ", Style::default().fg(Color::Cyan).add_modifier(Modifier::BOLD))),
        Line::from(Span::styled("| |/ / /_/ / _, _/ ___ |          ", Style::default().fg(Color::Cyan).add_modifier(Modifier::BOLD))),
        Line::from(Span::styled("|___/\\____/_/_|_/_/__|_|__  _   __", Style::default().fg(Color::Cyan).add_modifier(Modifier::BOLD))),
        Line::from(Span::styled("| |  / /  _/ ___//  _/ __ \\/ | / /", Style::default().fg(Color::Cyan).add_modifier(Modifier::BOLD))),
        Line::from(Span::styled("| | / // / \\__ \\ / // / / /  |/ / ", Style::default().fg(Color::Cyan).add_modifier(Modifier::BOLD))),
        Line::from(Span::styled("| |/ // / ___/ // // /_/ / /|  /  ", Style::default().fg(Color::Cyan).add_modifier(Modifier::BOLD))),
        Line::from(Span::styled("|___/___//____/___/\\____/_/ |_/   ", Style::default().fg(Color::Cyan).add_modifier(Modifier::BOLD))),
    ];

    let title_block = Block::default()
        .borders(Borders::NONE)
        .style(Style::default());
    let title = Paragraph::new(logo_lines)
        .block(title_block)
        .alignment(Alignment::Center);
    f.render_widget(title, chunks[1]);

    // Instruction
    let instr = Paragraph::new("Enter ticker(s) separated by spaces or commas (e.g. AAPL, MSFT, GOOGL):")
        .alignment(Alignment::Center)
        .style(Style::default().fg(Color::DarkGray));
    f.render_widget(instr, chunks[2]);

    // Text Input Box
    let input_focused = app.focus == InputFocus::TextInput;
    let input_border_color = if input_focused { Color::Yellow } else { Color::DarkGray };
    let input_block = Block::default()
        .borders(Borders::ALL)
        .title(" Tickers Input ")
        .border_style(Style::default().fg(input_border_color));

    // Blinking cursor simulation
    let display_text = if input_focused {
        if app.tick_count % 10 < 5 {
            format!("{}█", app.input_buffer)
        } else {
            format!("{} ", app.input_buffer)
        }
    } else {
        app.input_buffer.clone()
    };

    let input_para = Paragraph::new(display_text)
        .block(input_block)
        .style(Style::default().fg(if input_focused { Color::White } else { Color::Gray }));
    f.render_widget(input_para, chunks[3]);

    // Separator line
    let sep = Paragraph::new("⎯⎯⎯⎯⎯⎯⎯⎯⎯⎯⎯⎯⎯⎯⎯⎯⎯⎯⎯⎯⎯⎯⎯⎯⎯⎯⎯⎯⎯⎯⎯⎯⎯⎯⎯⎯⎯⎯⎯⎯⎯⎯⎯⎯⎯⎯⎯⎯⎯⎯⎯⎯⎯⎯⎯⎯⎯⎯")
        .alignment(Alignment::Center)
        .style(Style::default().fg(Color::DarkGray));
    f.render_widget(sep, chunks[4]);

    // Horizon Selector
    let hor_focused = app.focus == InputFocus::HorizonSelector;
    
    let hor_layout = Layout::default()
        .direction(Direction::Horizontal)
        .constraints([
            Constraint::Percentage(25),
            Constraint::Percentage(50),
            Constraint::Percentage(25),
        ])
        .split(chunks[5]);

    let buttons_layout = Layout::default()
        .direction(Direction::Horizontal)
        .constraints([
            Constraint::Ratio(1, 3),
            Constraint::Ratio(1, 3),
            Constraint::Ratio(1, 3),
        ])
        .split(hor_layout[1]);

    let render_button = |f: &mut Frame, rect: Rect, label: &str, horizon: Horizon, current: Horizon, block_focus: bool| {
        let is_selected = horizon == current;
        let mut style = Style::default();
        if is_selected {
            if block_focus {
                style = style.fg(Color::Black).bg(Color::Cyan).add_modifier(Modifier::BOLD);
            } else {
                style = style.fg(Color::Cyan).add_modifier(Modifier::BOLD);
            }
        } else {
            style = style.fg(Color::Gray);
        }
        let block = Block::default()
            .borders(Borders::ALL)
            .border_style(Style::default().fg(if is_selected && block_focus { Color::Yellow } else if is_selected { Color::Cyan } else { Color::DarkGray }));
        let para = Paragraph::new(label)
            .block(block)
            .style(style)
            .alignment(Alignment::Center);
        f.render_widget(para, rect);
    };

    render_button(f, buttons_layout[0], "Short-Term", Horizon::Short, app.horizon, hor_focused);
    render_button(f, buttons_layout[1], "Medium-Term", Horizon::Medium, app.horizon, hor_focused);
    render_button(f, buttons_layout[2], "Long-Term", Horizon::Long, app.horizon, hor_focused);

    // Horizon Explanation Panel
    let (horizon_title, horizon_desc, horizon_detail) = match app.horizon {
        Horizon::Short => (
            "Short-Term (Days to Weeks)",
            "Optimized for rapid price movements and near-term catalysts.",
            vec![
                "Emphasizes Risk (35%) and Macro Environment (25%) because short-term",
                "price action is driven by market conditions, volatility, and sentiment",
                "shifts. Fundamentals matter less over days-to-weeks because earnings",
                "and balance sheet strength take quarters to materialize.",
                "",
                "Best for: Swing trades, event-driven plays, earnings momentum.",
            ],
        ),
        Horizon::Medium => (
            "Medium-Term (Weeks to Months)",
            "Balanced approach across all analytical categories.",
            vec![
                "Evenly weights Valuation (25%) and Fundamentals (25%) alongside",
                "Macro (20%), Sentiment (15%), and Risk (15%). Over this timeframe,",
                "a stock's price begins to converge toward its intrinsic value, so",
                "both the company's financial health and market conditions matter.",
                "",
                "Best for: Position trades, quarterly outlook, sector rotation.",
            ],
        ),
        Horizon::Long => (
            "Long-Term (Months to Years)",
            "Focused on durable competitive advantages and financial strength.",
            vec![
                "Heavily weights Fundamentals (45%) because over multi-year periods,",
                "stock prices are driven by earnings growth, cash flow generation,",
                "and balance sheet quality. Valuation (20%) still matters to avoid",
                "overpaying. Sentiment and Risk carry less weight (10% each).",
                "",
                "Best for: Buy-and-hold investing, retirement portfolios, compounding.",
            ],
        ),
    };

    let explain_layout = Layout::default()
        .direction(Direction::Horizontal)
        .constraints([
            Constraint::Min(0),
            Constraint::Length(76),
            Constraint::Min(0),
        ])
        .split(chunks[7]);

    let mut explain_lines = vec![
        Line::from(vec![
            Span::styled(horizon_title, Style::default().fg(Color::Cyan).add_modifier(Modifier::BOLD)),
        ]),
        Line::from(Span::styled(horizon_desc, Style::default().fg(Color::White))),
        Line::from(""),
    ];
    for detail_line in horizon_detail {
        explain_lines.push(Line::from(Span::styled(detail_line, Style::default().fg(Color::DarkGray))));
    }

    let explain_block = Block::default()
        .borders(Borders::ALL)
        .title(" Why does this matter? ")
        .border_style(Style::default().fg(Color::DarkGray));
    let explain_para = Paragraph::new(explain_lines)
        .block(explain_block)
        .alignment(Alignment::Center);
    f.render_widget(explain_para, explain_layout[1]);

    // Quick Backtest callout (sits just above the hint bar)
    let quick_line = Line::from(vec![
        Span::styled(" X ", Style::default().fg(Color::Black).bg(Color::Yellow).add_modifier(Modifier::BOLD)),
        Span::styled(
            "  Quick Backtest — test VORA's algorithm against the S&P 500 using a built-in diversified 20-stock universe",
            Style::default().fg(Color::Yellow),
        ),
    ]);
    f.render_widget(
        Paragraph::new(quick_line).alignment(Alignment::Center),
        chunks[8],
    );

    // Bottom Hint Bar
    let hint_style = Style::default().fg(Color::Black).bg(Color::DarkGray);
    let hint_line = Line::from(vec![
        Span::styled(" Tab ", Style::default().fg(Color::Cyan).add_modifier(Modifier::BOLD)),
        Span::raw("Switch Field  |  "),
        Span::styled(" Left/Right ", Style::default().fg(Color::Yellow).add_modifier(Modifier::BOLD)),
        Span::raw("Change Horizon  |  "),
        Span::styled(" Enter ", Style::default().fg(Color::Green).add_modifier(Modifier::BOLD)),
        Span::raw("Analyze  |  "),
        Span::styled(" X ", Style::default().fg(Color::Yellow).add_modifier(Modifier::BOLD)),
        Span::raw("Quick Backtest  |  "),
        Span::styled(" Q ", Style::default().fg(Color::Red).add_modifier(Modifier::BOLD)),
        Span::raw("Quit"),
    ]);
    let hint = Paragraph::new(hint_line)
        .alignment(Alignment::Center)
        .style(hint_style);
    f.render_widget(hint, chunks[9]);
}
