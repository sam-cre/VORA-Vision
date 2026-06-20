use crate::app::App;
use crate::sim::BacktestResult;
use ratatui::{
    layout::{Alignment, Constraint, Direction, Layout, Rect},
    style::{Color, Modifier, Style},
    symbols,
    text::{Line, Span},
    widgets::{Axis, Block, Borders, Chart, Dataset, GraphType, Paragraph, Row, Table},
    Frame,
};

pub fn render(f: &mut Frame, app: &App) {
    let size = f.area();
    let bt = match &app.backtest {
        Some(b) => b,
        None => {
            let p = Paragraph::new("No backtest data. Press R to return to search.")
                .alignment(Alignment::Center)
                .style(Style::default().fg(Color::Red));
            f.render_widget(p, size);
            return;
        }
    };

    // Reserve a walk-forward row only when we have sub-period data (portfolio mode).
    let has_wf = !bt.subperiods.is_empty();
    let mut constraints = vec![
        Constraint::Length(3), // header
        Constraint::Min(6),    // chart
        Constraint::Length(10), // stats
    ];
    if has_wf {
        constraints.push(Constraint::Length(6)); // walk-forward
    }
    constraints.push(Constraint::Length(1)); // footer
    let chunks = Layout::default()
        .direction(Direction::Vertical)
        .constraints(constraints)
        .split(size);
    let footer_idx = chunks.len() - 1;

    let start = bt.strategy.equity_curve.first().map(|p| p.date);
    let end = bt.strategy.equity_curve.last().map(|p| p.date);
    let window = match (start, end) {
        (Some(s), Some(e)) => format!("{} -> {}", s, e),
        _ => "-".to_string(),
    };
    let identical = (bt.total_return_pct - bt.buy_hold_return_pct).abs() < 0.5;
    let header = if identical {
        format!(
            " VORA-VISION — Backtest: {} · {} · {}   [VORA stayed fully invested = Buy & Hold] ",
            bt.ticker, bt.horizon, window
        )
    } else {
        format!(" VORA-VISION — Backtest: {} · {} · {} ", bt.ticker, bt.horizon, window)
    };
    f.render_widget(
        Paragraph::new(Span::styled(
            header,
            Style::default().fg(Color::Cyan).add_modifier(Modifier::BOLD),
        ))
        .block(Block::default().borders(Borders::ALL).border_style(Style::default().fg(Color::Cyan)))
        .alignment(Alignment::Center),
        chunks[0],
    );

    render_chart(f, chunks[1], bt);
    render_stats(f, chunks[2], bt);
    if has_wf {
        render_walkforward(f, chunks[3], bt);
    }

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
        chunks[footer_idx],
    );
}

/// Walk-forward robustness: VORA vs SPY in each consecutive window, with a ✓/✗
/// showing whether the strategy's edge held in that window (not just on average).
fn render_walkforward(f: &mut Frame, rect: Rect, bt: &BacktestResult) {
    let hdr = Style::default().fg(Color::Cyan).add_modifier(Modifier::BOLD);
    let wins = bt.subperiods.iter().filter(|s| s.vora_pct > s.spy_pct).count();
    let total = bt.subperiods.len();

    let rows: Vec<Row> = bt.subperiods.iter().map(|s| {
        let beat = s.vora_pct > s.spy_pct;
        let mark = if beat { "✓ beat" } else { "✗ lag" };
        Row::new(vec![
            s.label.clone(),
            format!("{:+.1}%", s.vora_pct),
            format!("{:+.1}%", s.spy_pct),
            mark.to_string(),
        ])
        .style(Style::default().fg(if beat { Color::Green } else { Color::Red }))
    }).collect();

    let title = format!(" Walk-Forward Robustness — VORA beat SPY in {}/{} windows ", wins, total);
    let table = Table::new(
        rows,
        [Constraint::Percentage(40), Constraint::Percentage(20), Constraint::Percentage(20), Constraint::Percentage(20)],
    )
    .header(Row::new(vec!["Window (out-of-sample)", "VORA", "SPY", "Result"]).style(hdr))
    .block(
        Block::default()
            .borders(Borders::ALL)
            .title(title)
            .border_style(Style::default().fg(Color::DarkGray)),
    );
    f.render_widget(table, rect);
}

fn render_chart(f: &mut Frame, rect: Rect, bt: &BacktestResult) {
    let start_date = match bt.strategy.equity_curve.first() {
        Some(p) => p.date,
        None => return,
    };
    let end_date = bt.strategy.equity_curve.last().map(|p| p.date).unwrap_or(start_date);
    let to_xy = |date: chrono::NaiveDate, val: f64| ((date - start_date).num_days() as f64, val);

    let vora: Vec<(f64, f64)> = bt.strategy.equity_curve.iter().map(|p| to_xy(p.date, p.value)).collect();
    let bh: Vec<(f64, f64)> = bt.buy_hold_curve.iter().map(|(d, v)| to_xy(*d, *v)).collect();
    let spy: Vec<(f64, f64)> = bt
        .benchmark_curve
        .as_ref()
        .map(|c| c.iter().map(|(d, v)| to_xy(*d, *v)).collect())
        .unwrap_or_default();
    let rnd: Vec<(f64, f64)> = bt
        .random_curve
        .as_ref()
        .map(|c| c.iter().map(|(d, v)| to_xy(*d, *v)).collect())
        .unwrap_or_default();

    let mut ymin = f64::MAX;
    let mut ymax = f64::MIN;
    let mut xmax = 1.0_f64;
    for (x, y) in vora.iter().chain(bh.iter()).chain(spy.iter()).chain(rnd.iter()) {
        ymin = ymin.min(*y);
        ymax = ymax.max(*y);
        xmax = xmax.max(*x);
    }
    if !ymin.is_finite() || !ymax.is_finite() {
        return;
    }
    let pad = (ymax - ymin).max(1.0) * 0.05;
    let ylo = (ymin - pad).max(0.0);
    let yhi = ymax + pad;

    // Order matters: later datasets paint on top. Draw the benchmarks first and
    // VORA last so the strategy curve stays visible even when it sits exactly on
    // top of Buy & Hold (e.g. when VORA stays fully invested).
    let mut datasets = vec![Dataset::default()
        .name("Buy & Hold")
        .marker(symbols::Marker::Braille)
        .graph_type(GraphType::Line)
        .style(Style::default().fg(Color::Yellow))
        .data(&bh)];
    if !spy.is_empty() {
        datasets.push(
            Dataset::default()
                .name("SPY")
                .marker(symbols::Marker::Braille)
                .graph_type(GraphType::Line)
                .style(Style::default().fg(Color::Blue))
                .data(&spy),
        );
    }
    if !rnd.is_empty() {
        datasets.push(
            Dataset::default()
                .name("Random (med)")
                .marker(symbols::Marker::Braille)
                .graph_type(GraphType::Line)
                .style(Style::default().fg(Color::Magenta))
                .data(&rnd),
        );
    }
    datasets.push(
        Dataset::default()
            .name("VORA")
            .marker(symbols::Marker::Braille)
            .graph_type(GraphType::Line)
            .style(Style::default().fg(Color::Green))
            .data(&vora),
    );

    let chart = Chart::new(datasets)
        .block(
            Block::default()
                .borders(Borders::ALL)
                .title(" Equity Curve ($10,000 start) ")
                .border_style(Style::default().fg(Color::DarkGray)),
        )
        .x_axis(
            Axis::default()
                .style(Style::default().fg(Color::DarkGray))
                .bounds([0.0, xmax])
                .labels(vec![
                    Span::raw(start_date.format("%Y-%m").to_string()),
                    Span::raw(end_date.format("%Y-%m").to_string()),
                ]),
        )
        .y_axis(
            Axis::default()
                .style(Style::default().fg(Color::DarkGray))
                .bounds([ylo, yhi])
                .labels(vec![
                    Span::raw(format!("${:.0}", ylo)),
                    Span::raw(format!("${:.0}", (ylo + yhi) / 2.0)),
                    Span::raw(format!("${:.0}", yhi)),
                ]),
        );
    f.render_widget(chart, rect);
}

fn render_stats(f: &mut Frame, rect: Rect, bt: &BacktestResult) {
    let cols = Layout::default()
        .direction(Direction::Horizontal)
        .constraints([Constraint::Percentage(50), Constraint::Percentage(50)])
        .split(rect);

    let hdr = Style::default().fg(Color::Cyan).add_modifier(Modifier::BOLD);

    let perf = vec![
        Row::new(vec!["Total Return".to_string(), format!("{:+.1}%", bt.total_return_pct)]),
        Row::new(vec!["CAGR".to_string(), format!("{:+.1}%", bt.cagr_pct)]),
        Row::new(vec!["Sharpe".to_string(), format!("{:.2}", bt.sharpe)]),
        Row::new(vec!["Max Drawdown".to_string(), format!("{:.1}%", bt.max_drawdown_pct)]),
        Row::new(vec!["Time in Market".to_string(), format!("{:.0}%", bt.time_in_market_pct)]),
    ];
    let perf_table = Table::new(perf, [Constraint::Percentage(60), Constraint::Percentage(40)])
        .header(Row::new(vec!["Strategy (VORA)", ""]).style(hdr))
        .block(Block::default().borders(Borders::ALL).border_style(Style::default().fg(Color::DarkGray)));
    f.render_widget(perf_table, cols[0]);

    let spy_ret = bt
        .benchmark_return_pct
        .map(|v| format!("{:+.1}%", v))
        .unwrap_or_else(|| "N/A".to_string());
    let excess = bt
        .benchmark_return_pct
        .map(|spy| format!("{:+.1}%", bt.total_return_pct - spy))
        .unwrap_or_else(|| "N/A".to_string());

    let mut bench = vec![
        Row::new(vec!["Buy & Hold".to_string(), format!("{:+.1}%", bt.buy_hold_return_pct)]),
        Row::new(vec!["SPY".to_string(), spy_ret]),
        Row::new(vec!["Excess vs SPY".to_string(), excess]),
    ];
    // Random "monkey portfolio" benchmark (portfolio mode only).
    if let (Some(rand_ret), Some(beats)) = (bt.random_return_pct, bt.beats_random_pct) {
        bench.push(Row::new(vec![
            "Random (med)".to_string(),
            format!("{:+.1}%", rand_ret),
        ]));
        bench.push(
            Row::new(vec![
                "Beats random".to_string(),
                format!("{:.0}% of {}", beats, bt.random_trials),
            ])
            .style(
                Style::default()
                    .fg(if beats >= 50.0 { Color::Green } else { Color::Red })
                    .add_modifier(Modifier::BOLD),
            ),
        );
    }
    bench.push(Row::new(vec![
        "Win rate".to_string(),
        format!("{:.0}% ({} trips)", bt.hit_rate_pct, bt.round_trips),
    ]));
    bench.push(Row::new(vec!["Avg score".to_string(), format!("{:.1}", bt.avg_score)]));
    let bench_table = Table::new(bench, [Constraint::Percentage(60), Constraint::Percentage(40)])
        .header(Row::new(vec!["vs Benchmark", ""]).style(hdr))
        .block(Block::default().borders(Borders::ALL).border_style(Style::default().fg(Color::DarkGray)));
    f.render_widget(bench_table, cols[1]);
}
