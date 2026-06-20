use crate::app::App;
use crate::models::AnalysisResult;
use crate::scoring::normalize::*;
use crate::sim::Calibration;
use ratatui::{
    layout::{Alignment, Constraint, Direction, Layout, Rect},
    style::{Color, Modifier, Style},
    text::{Line, Span},
    widgets::{Block, Borders, List, ListItem, Paragraph, Row, Table, Tabs, Wrap},
    Frame,
};

fn truncate_cell(text: &str, width: usize) -> String {
    if text.chars().count() > width {
        if width > 3 {
            format!("{}...", text.chars().take(width - 3).collect::<String>())
        } else {
            text.chars().take(width).collect::<String>()
        }
    } else {
        text.to_string()
    }
}


pub fn render(f: &mut Frame, app: &App) {
    let size = f.area();
    if app.results.is_empty() || app.active_result_idx >= app.results.len() {
        return;
    }

    let result = &app.results[app.active_result_idx];

    let main_chunks = Layout::default()
        .direction(Direction::Vertical)
        .constraints([
            Constraint::Length(3),
            Constraint::Length(2),
            Constraint::Min(0),
            Constraint::Length(1),
        ])
        .split(size);

    // Header
    let header_chunks = Layout::default()
        .direction(Direction::Horizontal)
        .constraints([Constraint::Percentage(50), Constraint::Percentage(50)])
        .split(main_chunks[0]);

    let title_block = Block::default().borders(Borders::ALL).border_style(Style::default().fg(Color::Cyan));
    let title_text = format!(" VORA-VISION — Report for {} ", result.ticker);
    f.render_widget(
        Paragraph::new(Span::styled(&title_text, Style::default().fg(Color::Cyan).add_modifier(Modifier::BOLD)))
            .block(title_block).alignment(Alignment::Left),
        header_chunks[0],
    );

    let horizon_block = Block::default().borders(Borders::ALL).border_style(Style::default().fg(Color::Cyan));
    let horizon_text = format!(" Horizon: {} ", result.horizon);
    f.render_widget(
        Paragraph::new(Span::styled(&horizon_text, Style::default().fg(Color::Cyan).add_modifier(Modifier::BOLD)))
            .block(horizon_block).alignment(Alignment::Right),
        header_chunks[1],
    );

    // Tabs
    let tab_titles = vec!["Overview", "Valuation", "Fundamentals", "Macro Env", "Sentiment", "Risk Profile", "Methodology", "Metric Guide"];
    let tabs = Tabs::new(tab_titles)
        .block(Block::default().borders(Borders::BOTTOM))
        .select(app.active_tab)
        .style(Style::default().fg(Color::Gray))
        .highlight_style(Style::default().fg(Color::Yellow).add_modifier(Modifier::BOLD | Modifier::UNDERLINED));
    f.render_widget(tabs, main_chunks[1]);

    // Content: Left Signal Panel, Right Detail Panel
    let content_chunks = Layout::default()
        .direction(Direction::Horizontal)
        .constraints([Constraint::Percentage(30), Constraint::Percentage(70)])
        .split(main_chunks[2]);

    render_signal_panel(f, content_chunks[0], result, app.calibration.as_ref());
    render_detail_panel(f, content_chunks[1], result, app.active_tab, app.calibration.as_ref());

    // Footer
    let mut footer_spans = vec![
        Span::styled(" ←/→ ", Style::default().fg(Color::Cyan).add_modifier(Modifier::BOLD)),
        Span::raw("Tabs  "),
    ];
    if app.results.len() >= 2 {
        footer_spans.push(Span::styled(" C ", Style::default().fg(Color::Yellow).add_modifier(Modifier::BOLD)));
        footer_spans.push(Span::raw("Compare  "));
    }
    footer_spans.extend([
        Span::styled(" B ", Style::default().fg(Color::Green).add_modifier(Modifier::BOLD)),
        Span::raw("Backtest  "),
        Span::styled(" P ", Style::default().fg(Color::Green).add_modifier(Modifier::BOLD)),
        Span::raw("Paper-trade  "),
        Span::styled(" K ", Style::default().fg(Color::Green).add_modifier(Modifier::BOLD)),
        Span::raw("Calibrate  "),
        Span::styled(" V ", Style::default().fg(Color::Cyan).add_modifier(Modifier::BOLD)),
        Span::raw("Portfolio  "),
        Span::styled(" R ", Style::default().fg(Color::Red).add_modifier(Modifier::BOLD)),
        Span::raw("Reset  "),
        Span::styled(" Q ", Style::default().fg(Color::Red).add_modifier(Modifier::BOLD)),
        Span::raw("Quit"),
    ]);
    let footer_text = Line::from(footer_spans);
    f.render_widget(
        Paragraph::new(footer_text).alignment(Alignment::Center).style(Style::default().fg(Color::Black).bg(Color::DarkGray)),
        main_chunks[3],
    );
}

fn render_signal_panel(f: &mut Frame, rect: Rect, result: &AnalysisResult, cal: Option<&Calibration>) {
    let block = Block::default().borders(Borders::ALL).title(" Signal ").border_style(Style::default().fg(Color::DarkGray));
    f.render_widget(block, rect);

    let inner = rect.inner(ratatui::layout::Margin { horizontal: 1, vertical: 1 });
    let layout = Layout::default()
        .direction(Direction::Vertical)
        .constraints([
            Constraint::Length(1),
            Constraint::Length(1), // Signal
            Constraint::Length(1),
            Constraint::Length(1), // Score
            Constraint::Length(1), // Coverage
            Constraint::Length(1), // Odds (calibrated)
            Constraint::Length(1),
            Constraint::Min(0),   // Bars
        ])
        .split(inner);

    let sig_text = format!("  {}  ", result.signal);
    let sig_color = result.signal.color();
    f.render_widget(
        Paragraph::new(Span::styled(sig_text, Style::default().fg(Color::Black).bg(sig_color).add_modifier(Modifier::BOLD)))
            .alignment(Alignment::Center),
        layout[1],
    );

    let score_text = format!("Score: {:.1} / 100", result.composite_score);
    f.render_widget(
        Paragraph::new(Span::styled(score_text, Style::default().fg(Color::White).add_modifier(Modifier::BOLD)))
            .alignment(Alignment::Center),
        layout[3],
    );

    let cov_text = format!("Coverage: {:.0}%", result.confidence_score);
    f.render_widget(
        Paragraph::new(Span::styled(cov_text, Style::default().fg(Color::DarkGray)))
            .alignment(Alignment::Center),
        layout[4],
    );

    // Honest odds: how stocks at this score actually fared historically — always
    // shown WITH the sample size, and greyed out + caveated when n is too small to
    // trust (the whole point is to not present noise as a probability).
    const MIN_RELIABLE_N: usize = 30;
    let odds_line = match cal.and_then(|c| c.bucket_for(result.composite_score)) {
        Some(b) => {
            let low = b.n < MIN_RELIABLE_N;
            let color = if low {
                Color::DarkGray
            } else if b.win_rate >= 50.0 {
                Color::Green
            } else {
                Color::Red
            };
            let txt = if low {
                format!("{:.0}% up · n={} (low)", b.win_rate, b.n)
            } else {
                format!("{:.0}% up · {:+.0}% · n={}", b.win_rate, b.median, b.n)
            };
            Line::from(Span::styled(txt, color))
        }
        None => Line::from(Span::styled("Hist odds: calibrating…", Style::default().fg(Color::DarkGray))),
    };
    f.render_widget(Paragraph::new(odds_line).alignment(Alignment::Center), layout[5]);

    // Draw category bars
    let scores = [
        ("V", result.valuation.raw_score),
        ("F", result.fundamentals.raw_score),
        ("M", result.macro_env.raw_score),
        ("S", result.sentiment.raw_score),
        ("R", result.risk.raw_score),
    ];
    draw_bars(f, layout[7], &scores);
}


fn draw_bars(f: &mut Frame, rect: Rect, scores: &[(&str, f64)]) {
    let h = rect.height as usize;
    if h < 3 { return; }
    let chart_h = h.saturating_sub(1);

    for row in 0..chart_h {
        let y = rect.y + row as u16;
        let threshold = (chart_h - row) as f64 / chart_h as f64 * 100.0;
        let mut spans = Vec::new();
        spans.push(Span::raw(" "));
        for &(_label, score) in scores {
            let color = if score >= 65.0 { Color::Green } else if score >= 41.0 { Color::Yellow } else { Color::Red };
            if score >= threshold {
                spans.push(Span::styled("██", Style::default().fg(color)));
            } else {
                spans.push(Span::raw("  "));
            }
            spans.push(Span::raw(" "));
        }
        f.render_widget(Paragraph::new(Line::from(spans)), Rect::new(rect.x, y, rect.width, 1));
    }

    let mut label_spans = vec![Span::raw(" ")];
    for &(label, _) in scores {
        label_spans.push(Span::styled(format!("{} ", label), Style::default().fg(Color::Cyan).add_modifier(Modifier::BOLD)));
        label_spans.push(Span::raw(" "));
    }
    f.render_widget(Paragraph::new(Line::from(label_spans)), Rect::new(rect.x, rect.y + chart_h as u16, rect.width, 1));
}

fn render_detail_panel(f: &mut Frame, rect: Rect, result: &AnalysisResult, tab: usize, cal: Option<&Calibration>) {
    let block = Block::default().borders(Borders::ALL).title(" Details ").border_style(Style::default().fg(Color::DarkGray));
    f.render_widget(block, rect);
    let inner = rect.inner(ratatui::layout::Margin { horizontal: 1, vertical: 1 });

    match tab {
        0 => render_overview(f, inner, result, cal),
        1 => render_valuation(f, inner, result),
        2 => render_fundamentals(f, inner, result),
        3 => render_macro(f, inner, result),
        4 => render_sentiment(f, inner, result),
        5 => render_risk(f, inner, result),
        6 => render_methodology(f, inner),
        7 => render_metric_guide(f, inner, result),
        _ => {}
    }
}

fn render_overview(f: &mut Frame, rect: Rect, result: &AnalysisResult, cal: Option<&Calibration>) {
    let layout = Layout::default()
        .direction(Direction::Vertical)
        .constraints([Constraint::Length(10), Constraint::Length(3), Constraint::Min(0)])
        .split(rect);

    render_expected_band(f, layout[1], result, cal);

    let hdr = Style::default().fg(Color::Cyan).add_modifier(Modifier::BOLD);

    let cats = [
        ("Valuation", &result.valuation),
        ("Fundamentals", &result.fundamentals),
        ("Macro Environment", &result.macro_env),
        ("Sentiment", &result.sentiment),
        ("Risk Profile", &result.risk),
    ];

    // "Eff.Wt%" is each category's *actual* share of the composite (weight × coverage,
    // normalized) — unlike a nominal raw×weight column, these reconcile with the score.
    let total_cw: f64 = cats.iter().map(|(_, c)| c.weight * c.coverage).sum();
    let cell_store: Vec<[String; 5]> = cats.iter().map(|(name, cat)| {
        let eff = if total_cw > 1e-9 { cat.weight * cat.coverage / total_cw * 100.0 } else { 0.0 };
        [
            name.to_string(),
            format!("{:.1}", cat.raw_score),
            format!("{:.0}%", cat.weight * 100.0),
            format!("{:.0}%", eff),
            format!("{:.0}%", cat.coverage * 100.0),
        ]
    }).collect();
    let comp = format!("{:.1}", result.composite_score);

    let mut rows: Vec<Row> = cell_store.iter().map(|c| {
        Row::new(vec![c[0].as_str(), c[1].as_str(), c[2].as_str(), c[3].as_str(), c[4].as_str()])
    }).collect();
    rows.push(Row::new(vec!["", "", "", "", ""]));
    rows.push(
        Row::new(vec!["COMPOSITE", comp.as_str(), "", "", ""])
            .style(Style::default().fg(Color::Yellow).add_modifier(Modifier::BOLD)),
    );

    let table = Table::new(rows, [
        Constraint::Percentage(34), Constraint::Percentage(16), Constraint::Percentage(16),
        Constraint::Percentage(17), Constraint::Percentage(17),
    ]).header(Row::new(vec!["Category", "Raw", "Weight", "Eff.Wt", "Coverage"]).style(hdr));
    f.render_widget(table, layout[0]);

    // Bottom: "Why this signal" narrative (left) + data-coverage detail (right).
    let bottom = Layout::default()
        .direction(Direction::Horizontal)
        .constraints([Constraint::Percentage(60), Constraint::Percentage(40)])
        .split(layout[2]);

    render_why(f, bottom[0], result);

    // Data-coverage panel: explain how coverage affects the composite and flag gaps.
    let any_missing = cats.iter().any(|(_, c)| c.coverage < 0.999);
    let mut lines: Vec<Line> = Vec::new();
    if any_missing {
        lines.push(Line::from(Span::styled(
            "Composite = coverage-weighted average of category scores, then shrunk toward neutral (50) in proportion to overall coverage — thin-data names stay near HOLD.",
            Style::default().fg(Color::DarkGray),
        )));
        for (name, cat) in cats.iter() {
            if cat.coverage < 0.999 {
                lines.push(Line::from(Span::styled(
                    format!("[!] {} — {:.0}% coverage", name, cat.coverage * 100.0),
                    Style::default().fg(Color::Yellow),
                )));
            }
        }
    } else {
        lines.push(Line::from(Span::styled(
            "Full data coverage across all categories.",
            Style::default().fg(Color::Green),
        )));
    }

    let panel = Paragraph::new(lines)
        .block(Block::default().title(" Data Coverage ").borders(Borders::ALL).border_style(Style::default().fg(Color::DarkGray)))
        .wrap(Wrap { trim: true });
    f.render_widget(panel, bottom[1]);
}

/// Plain-English "why" panel: the signal's main drivers, detractors, and what
/// would flip it — all derived from the category contributions (scoring::explain).
fn render_why(f: &mut Frame, rect: Rect, result: &AnalysisResult) {
    let ex = crate::scoring::explain::explain(result);
    let mut lines: Vec<Line> = Vec::new();

    lines.push(Line::from(Span::styled(
        ex.headline,
        Style::default().fg(Color::White).add_modifier(Modifier::BOLD),
    )));
    lines.push(Line::from(""));

    if !ex.drivers.is_empty() {
        lines.push(Line::from(Span::styled("Drivers", Style::default().fg(Color::Green).add_modifier(Modifier::BOLD))));
        for d in ex.drivers.iter().take(3) {
            let detail = d.detail.as_deref().map(|s| format!(" — {}", s)).unwrap_or_default();
            lines.push(Line::from(Span::styled(
                format!("  + {} ({:+.1} pts){}", d.name, d.contribution, detail),
                Style::default().fg(Color::Gray),
            )));
        }
    }
    if !ex.detractors.is_empty() {
        lines.push(Line::from(Span::styled("Detractors", Style::default().fg(Color::Red).add_modifier(Modifier::BOLD))));
        for d in ex.detractors.iter().take(2) {
            let detail = d.detail.as_deref().map(|s| format!(" — {}", s)).unwrap_or_default();
            lines.push(Line::from(Span::styled(
                format!("  - {} ({:+.1} pts){}", d.name, d.contribution, detail),
                Style::default().fg(Color::Gray),
            )));
        }
    }
    lines.push(Line::from(""));
    lines.push(Line::from(Span::styled(ex.flip, Style::default().fg(Color::DarkGray))));

    let panel = Paragraph::new(lines)
        .block(Block::default().title(" Why this signal ").borders(Borders::ALL).border_style(Style::default().fg(Color::DarkGray)))
        .wrap(Wrap { trim: true });
    f.render_widget(panel, rect);
}

/// Inline calibrated forward-return expectation for the current composite score.
fn render_expected_band(f: &mut Frame, rect: Rect, result: &AnalysisResult, cal: Option<&Calibration>) {
    let line = match cal {
        Some(c) => match c.bucket_for(result.composite_score) {
            Some(b) => {
                let low = b.n < 30;
                let caveat = if low { "  ·  ⚠ small sample — treat as indicative only" } else { "" };
                Line::from(vec![
                    Span::styled("Calibrated expectation: ", Style::default().fg(Color::Gray)),
                    Span::styled(
                        format!("{:+.1}%", b.mean),
                        Style::default()
                            .fg(if low { Color::DarkGray } else if b.mean >= 0.0 { Color::Green } else { Color::Red })
                            .add_modifier(Modifier::BOLD),
                    ),
                    Span::raw(format!(
                        " avg over {} mo  ·  middle 50%: {:+.0}% to {:+.0}%  ·  {:.0}% positive  ·  n={}  (band {:.0}-{:.0}){}",
                        c.horizon_months, b.p25, b.p75, b.win_rate, b.n, b.lo, b.hi, caveat
                    )),
                ])
            }
            None => Line::from(Span::styled(
                "Calibrated expectation: no historical observations in this score band.",
                Style::default().fg(Color::DarkGray),
            )),
        },
        None => Line::from(Span::styled(
            "Forward expectation: calibrating against history…  (press K for the full table)",
            Style::default().fg(Color::DarkGray),
        )),
    };
    f.render_widget(
        Paragraph::new(line)
            .block(Block::default().title(" Forward Expectation ").borders(Borders::ALL).border_style(Style::default().fg(Color::DarkGray)))
            .wrap(Wrap { trim: true }),
        rect,
    );
}

fn render_valuation(f: &mut Frame, rect: Rect, result: &AnalysisResult) {
    let layout = Layout::default().direction(Direction::Vertical)
        .constraints([Constraint::Length(7), Constraint::Min(0)]).split(rect);
    let hdr = Style::default().fg(Color::Cyan).add_modifier(Modifier::BOLD);

    let pe_val = result.yahoo.pe_ratio.map_or("N/A".into(), |v| format!("{:.1}", v));
    let pe_sec = result.finnhub.sector_pe_avg.map_or("N/A".into(), |v| format!("{:.1}", v));
    let pe_score = result.yahoo.pe_ratio.map_or("—".into(), |pe| format!("{:.1}", normalize_pe(pe, result.finnhub.sector_pe_avg)));
    let ps_val = result.yahoo.ps_ratio.map_or("N/A".into(), |v| format!("{:.1}", v));
    let ps_sec = result.finnhub.sector_ps_avg.map_or("N/A".into(), |v| format!("{:.1}", v));
    let ps_score = result.yahoo.ps_ratio.map_or("—".into(), |ps| format!("{:.1}", normalize_ps(ps, result.finnhub.sector_ps_avg)));
    let pb_val = result.yahoo.pb_ratio.map_or("N/A".into(), |v| format!("{:.1}", v));
    let pb_score = result.yahoo.pb_ratio.map_or("—".into(), |pb| format!("{:.1}", normalize_pb(pb, result.yahoo.sector.as_deref())));

    let rows = vec![
        Row::new(vec![Cell::from("P/E"), Cell::from(pe_val.as_str()), Cell::from(pe_sec.as_str()), Cell::from(pe_score.as_str())]),
        Row::new(vec![Cell::from("P/S"), Cell::from(ps_val.as_str()), Cell::from(ps_sec.as_str()), Cell::from(ps_score.as_str())]),
        Row::new(vec![Cell::from("P/B"), Cell::from(pb_val.as_str()), Cell::from("—"), Cell::from(pb_score.as_str())]),
    ];
    let table = Table::new(rows, [Constraint::Percentage(30), Constraint::Percentage(25), Constraint::Percentage(25), Constraint::Percentage(20)])
        .header(Row::new(vec!["Metric", "Value", "Sector Avg", "Score"]).style(hdr));
    f.render_widget(table, layout[0]);
    render_notes(f, layout[1], &result.valuation.notes);
}

fn render_fundamentals(f: &mut Frame, rect: Rect, result: &AnalysisResult) {
    let layout = Layout::default().direction(Direction::Vertical)
        .constraints([Constraint::Length(9), Constraint::Min(0)]).split(rect);
    let hdr = Style::default().fg(Color::Cyan).add_modifier(Modifier::BOLD);

    let eps_val_source = result.edgar.actual_eps_growth_yoy.or(result.yahoo.eps_growth_yoy);
    let eps_v = eps_val_source.map_or("N/A".into(), |v| format!("{:.1}%", v));
    let eps_s = eps_val_source.map_or("—".into(), |v| format!("{:.1}", normalize_eps_growth(v)));
    let rev_v = result.yahoo.revenue_growth_yoy.map_or("N/A".into(), |v| format!("{:.1}%", v));
    let rev_s = result.yahoo.revenue_growth_yoy.map_or("—".into(), |v| format!("{:.1}", normalize_rev_growth(v)));
    let fcf_v = result.edgar.fcf_latest.map_or("N/A".into(), |v| format!("${:.0}M", v));
    let fcf_s = result.edgar.fcf_latest.map_or("—".into(), |fl| format!("{:.1}", normalize_fcf(fl, result.yahoo.market_cap)));
    let fcfg_v = result.edgar.fcf_growth_yoy.map_or("N/A".into(), |v| format!("{:.1}%", v));
    let fcfg_s = result.edgar.fcf_growth_yoy.map_or("—".into(), |v| format!("{:.1}", normalize_fcf_growth(v)));
    let gm_v = result.yahoo.gross_margin.map_or("N/A".into(), |v| format!("{:.1}%", v));
    let gm_s = result.yahoo.gross_margin.map_or("—".into(), |gm| format!("{:.1}", normalize_gross_margin(gm, result.yahoo.sector.as_deref())));
    let om_v = result.yahoo.operating_margin.map_or("N/A".into(), |v| format!("{:.1}%", v));
    let om_s = result.yahoo.operating_margin.map_or("—".into(), |om| format!("{:.1}", normalize_operating_margin(om, result.yahoo.sector.as_deref())));

    let rows = vec![
        Row::new(vec![Cell::from("EPS Growth"), Cell::from(eps_v.as_str()), Cell::from(eps_s.as_str())]),
        Row::new(vec![Cell::from("Revenue Growth"), Cell::from(rev_v.as_str()), Cell::from(rev_s.as_str())]),
        Row::new(vec![Cell::from("FCF Latest"), Cell::from(fcf_v.as_str()), Cell::from(fcf_s.as_str())]),
        Row::new(vec![Cell::from("FCF Growth"), Cell::from(fcfg_v.as_str()), Cell::from(fcfg_s.as_str())]),
        Row::new(vec![Cell::from("Gross Margin"), Cell::from(gm_v.as_str()), Cell::from(gm_s.as_str())]),
        Row::new(vec![Cell::from("Operating Margin"), Cell::from(om_v.as_str()), Cell::from(om_s.as_str())]),
    ];
    let table = Table::new(rows, [Constraint::Percentage(40), Constraint::Percentage(35), Constraint::Percentage(25)])
        .header(Row::new(vec!["Metric", "Value", "Score"]).style(hdr));
    f.render_widget(table, layout[0]);
    render_notes(f, layout[1], &result.fundamentals.notes);
}

fn render_macro(f: &mut Frame, rect: Rect, result: &AnalysisResult) {
    let layout = Layout::default().direction(Direction::Vertical)
        .constraints([Constraint::Length(8), Constraint::Min(0)]).split(rect);
    let hdr = Style::default().fg(Color::Cyan).add_modifier(Modifier::BOLD);

    let fed_v = result.fred.fed_funds_rate.map_or("N/A".into(), |v| format!("{:.2}%", v));
    let fed_s = result.fred.fed_funds_rate.map_or("—".into(), |v| format!("{:.1}", normalize_fed_funds(v)));
    let cpi_v = result.fred.cpi_yoy_change.map_or("N/A".into(), |v| format!("{:.2}%", v));
    let cpi_trend = result.fred.cpi_trend.clone().unwrap_or_else(|| "N/A".into());
    let cpi_s = result.fred.cpi_yoy_change.map_or("—".into(), |c| format!("{:.1}", normalize_cpi(c, result.fred.cpi_trend.as_deref())));
    let sg_v = result.finnhub.sector_growth_score.map_or("N/A".into(), |v| format!("{:.1}", v));
    let sg_s = result.finnhub.sector_growth_score.map_or("—".into(), |v| format!("{:.1}", v));
    let yc_v = result.fred.yield_curve_spread.map_or("N/A".into(), |v| format!("{:+.2}%", v));
    let yc_trend: String = result.fred.yield_curve_spread.map_or("—".into(), |v| if v < 0.0 { "Inverted".into() } else { "Normal".into() });
    let yc_s = result.fred.yield_curve_spread.map_or("—".into(), |v| format!("{:.1}", normalize_yield_curve(v)));
    let un_v = result.fred.unemployment_rate.map_or("N/A".into(), |v| format!("{:.1}%", v));
    let un_s = result.fred.unemployment_rate.map_or("—".into(), |v| format!("{:.1}", normalize_unemployment(v)));
    let vix_v = result.fred.vix.map_or("N/A".into(), |v| format!("{:.1}", v));
    let vix_s = result.fred.vix.map_or("—".into(), |v| format!("{:.1}", normalize_vix(v)));

    let rows = vec![
        Row::new(vec![Cell::from("Fed Funds Rate"), Cell::from(fed_v.as_str()), Cell::from("—"), Cell::from(fed_s.as_str())]),
        Row::new(vec![Cell::from("CPI YoY"), Cell::from(cpi_v.as_str()), Cell::from(cpi_trend.as_str()), Cell::from(cpi_s.as_str())]),
        Row::new(vec![Cell::from("Yield Curve 10Y-2Y"), Cell::from(yc_v.as_str()), Cell::from(yc_trend.as_str()), Cell::from(yc_s.as_str())]),
        Row::new(vec![Cell::from("Unemployment"), Cell::from(un_v.as_str()), Cell::from("—"), Cell::from(un_s.as_str())]),
        Row::new(vec![Cell::from("VIX (Volatility)"), Cell::from(vix_v.as_str()), Cell::from("—"), Cell::from(vix_s.as_str())]),
        Row::new(vec![Cell::from("Sector Growth"), Cell::from(sg_v.as_str()), Cell::from("—"), Cell::from(sg_s.as_str())]),
    ];
    let table = Table::new(rows, [Constraint::Percentage(30), Constraint::Percentage(25), Constraint::Percentage(20), Constraint::Percentage(25)])
        .header(Row::new(vec!["Metric", "Value", "Trend", "Score"]).style(hdr));
    f.render_widget(table, layout[0]);
    render_notes(f, layout[1], &result.macro_env.notes);
}

fn render_sentiment(f: &mut Frame, rect: Rect, result: &AnalysisResult) {
    let layout = Layout::default().direction(Direction::Vertical)
        .constraints([Constraint::Length(7), Constraint::Min(0)]).split(rect);
    let hdr = Style::default().fg(Color::Cyan).add_modifier(Modifier::BOLD);

    // News sentiment, mirroring the engine's CNN Fear & Greed fallback.
    let (news_v, news_s) = match result.finnhub.news_sentiment_score {
        Some(s) => (format!("{:.2}", s), format!("{:.1}", normalize_sentiment(s))),
        None => match result.fred.fear_and_greed {
            Some(fg) => (format!("F&G {:.0} (proxy)", fg), format!("{:.1}", fg)),
            None => ("N/A".to_string(), "—".to_string()),
        },
    };
    let shares_out = match (result.yahoo.market_cap, result.yahoo.price) {
        (Some(mc), Some(p)) if p > 0.0 => Some(mc / p),
        _ => None,
    };
    let ins_v = result.finnhub.insider_net_shares_3m.map_or("N/A".into(), |v| format!("{:+}", v));
    let ins_s = result.finnhub.insider_net_shares_3m.map_or("—".into(), |v| format!("{:.1}", normalize_insider(v, shares_out)));
    let inst_v = result.yahoo.institutional_ownership_percent.map_or("N/A".into(), |v| format!("{:.1}%", v));
    let inst_s = result.yahoo.institutional_ownership_percent.map_or("—".into(), |p| format!("{:.1}", normalize_institutional_ownership(p)));
    let mom_v = result.yahoo.price_52w_change_pct.map_or("N/A".into(), |v| format!("{:+.1}%", v));
    let mom_s = result.yahoo.price_52w_change_pct.map_or("—".into(), |c| format!("{:.1}", normalize_price_momentum(c)));

    let rows = vec![
        Row::new(vec![Cell::from("News Sentiment"), Cell::from(news_v.as_str()), Cell::from(news_s.as_str())]),
        Row::new(vec![Cell::from("Insider Trading (90d)"), Cell::from(ins_v.as_str()), Cell::from(ins_s.as_str())]),
        Row::new(vec![Cell::from("Institutional Own."), Cell::from(inst_v.as_str()), Cell::from(inst_s.as_str())]),
        Row::new(vec![Cell::from("Price Mom (52W)"), Cell::from(mom_v.as_str()), Cell::from(mom_s.as_str())]),
    ];
    let table = Table::new(rows, [Constraint::Percentage(45), Constraint::Percentage(30), Constraint::Percentage(25)])
        .header(Row::new(vec!["Metric", "Value", "Score"]).style(hdr));
    f.render_widget(table, layout[0]);
    render_notes(f, layout[1], &result.sentiment.notes);
}

fn render_risk(f: &mut Frame, rect: Rect, result: &AnalysisResult) {
    let layout = Layout::default().direction(Direction::Vertical)
        .constraints([Constraint::Length(8), Constraint::Min(0)]).split(rect);
    let hdr = Style::default().fg(Color::Cyan).add_modifier(Modifier::BOLD);

    let sh_v = result.yahoo.short_interest_percent.map_or("N/A".into(), |v| format!("{:.2}%", v));
    let sh_s = result.yahoo.short_interest_percent.map_or("—".into(), |v| format!("{:.1}", normalize_short_interest(v)));
    let ipo_cnt = result.finnhub.recent_competitor_ipos.len();
    let ipo_v = format!("{}", ipo_cnt);
    let ipo_s = format!("{:.1}", normalize_competitor_ipos(ipo_cnt));
    let de_v = result.edgar.debt_to_equity.map_or("N/A".into(), |v| format!("{:.2}", v));
    let de_s = result.edgar.debt_to_equity.map_or("—".into(), |de| format!("{:.1}", normalize_debt_to_equity(de, result.yahoo.sector.as_deref())));
    let ic_v = result.edgar.interest_coverage_ratio.map_or("N/A".into(), |v| format!("{:.1}x", v));
    let ic_s = result.edgar.interest_coverage_ratio.map_or("—".into(), |v| format!("{:.1}", normalize_interest_coverage(v)));
    let earn_v = result.finnhub.next_earnings_days.map_or("N/A".into(), |d| format!("in {}d", d));
    let earn_s = result.finnhub.next_earnings_days.map_or("—".into(), |d| format!("{:.1}", normalize_earnings_proximity(d)));

    let rows = vec![
        Row::new(vec![Cell::from("Short Interest %"), Cell::from(sh_v.as_str()), Cell::from(sh_s.as_str())]),
        Row::new(vec![Cell::from("Competitor IPOs"), Cell::from(ipo_v.as_str()), Cell::from(ipo_s.as_str())]),
        Row::new(vec![Cell::from("Debt/Equity"), Cell::from(de_v.as_str()), Cell::from(de_s.as_str())]),
        Row::new(vec![Cell::from("Interest Coverage"), Cell::from(ic_v.as_str()), Cell::from(ic_s.as_str())]),
        Row::new(vec![Cell::from("Next Earnings"), Cell::from(earn_v.as_str()), Cell::from(earn_s.as_str())]),
    ];
    let table = Table::new(rows, [Constraint::Percentage(45), Constraint::Percentage(30), Constraint::Percentage(25)])
        .header(Row::new(vec!["Risk Metric", "Value", "Score"]).style(hdr));
    f.render_widget(table, layout[0]);
    render_notes(f, layout[1], &result.risk.notes);
}

fn render_notes(f: &mut Frame, rect: Rect, notes: &[String]) {
    let block = Block::default()
        .title(" Notes ")
        .borders(Borders::ALL)
        .border_style(Style::default().fg(Color::DarkGray));
    
    let inner_rect = rect.inner(ratatui::layout::Margin { horizontal: 1, vertical: 1 });
    let max_items = inner_rect.height as usize;
    let max_width = inner_rect.width as usize;

    let mut items: Vec<ListItem> = if notes.is_empty() {
        let text = "No exceptional observations for this category.";
        vec![ListItem::new(truncate_cell(text, max_width))]
    } else {
        notes.iter().map(|n| {
            let text = format!("• {}", n);
            ListItem::new(truncate_cell(&text, max_width))
        }).collect()
    };

    if items.len() > max_items && max_items > 0 {
        items.truncate(max_items);
        // We don't add a new line of dots anymore, just let the last line be the last line.
    }

    let list = List::new(items).block(block).style(Style::default().fg(Color::Gray));
    f.render_widget(list, rect);
}

fn render_methodology(f: &mut Frame, rect: Rect) {
    let chunks = Layout::default()
        .direction(Direction::Vertical)
        .constraints([
            Constraint::Length(6), // Section 1: Weights
            Constraint::Length(1), // Spacer
            Constraint::Min(0),    // Section 2: Normalization
        ])
        .split(rect);

    // Section 1: Horizon Weights
    let weights_hdr = Style::default().fg(Color::Cyan).add_modifier(Modifier::BOLD);
    let weights_rows = vec![
        Row::new(vec!["Short-Term", "10%", "10%", "25%", "20%", "35%"]),
        Row::new(vec!["Medium-Term", "25%", "25%", "20%", "15%", "15%"]),
        Row::new(vec!["Long-Term", "20%", "45%", "15%", "10%", "10%"]),
    ];

    let weights_limit = chunks[0].height.saturating_sub(3) as usize;
    let mut visible_weights = weights_rows.clone();
    if weights_limit < weights_rows.len() && weights_limit > 0 {
        visible_weights.truncate(weights_limit);
    } else if weights_limit == 0 {
        visible_weights.clear();
    }

    let weights_table = Table::new(visible_weights, [
        Constraint::Percentage(20),
        Constraint::Percentage(16),
        Constraint::Percentage(16),
        Constraint::Percentage(16),
        Constraint::Percentage(16),
        Constraint::Percentage(16),
    ])
    .header(Row::new(vec!["Horizon", "Valuation", "Fundamentals", "Macro", "Sentiment", "Risk"]).style(weights_hdr))
    .block(Block::default().title(" Category Weights by Horizon ").borders(Borders::ALL).border_style(Style::default().fg(Color::DarkGray)));
    f.render_widget(weights_table, chunks[0]);

    // Section 2: Normalization Formulas
    let norm_hdr = Style::default().fg(Color::Cyan).add_modifier(Modifier::BOLD);
    
    let inner_w = chunks[2].width.saturating_sub(2) as f64;
    let col0_w = ((inner_w * 0.20) as usize).saturating_sub(2);
    let col1_w = ((inner_w * 0.35) as usize).saturating_sub(2);
    let col2_w = ((inner_w * 0.45) as usize).saturating_sub(2);

    let make_norm_row = |c0: &str, c1: &str, c2: &str| {
        Row::new(vec![
            Cell::from(truncate_cell(c0, col0_w)),
            Cell::from(truncate_cell(c1, col1_w)),
            Cell::from(truncate_cell(c2, col2_w)),
        ])
    };

    let norm_rows = vec![
        make_norm_row("P/E Ratio", "<=avg: 100-(PE/Avg)*50; above: tapers", "PE=Avg->50, 2x->33, 4x->0. Default avg 20"),
        make_norm_row("P/S Ratio", "<=avg: 100-(PS/Avg)*50; above: tapers", "PS=Avg->50, 2x->33, 4x->0. Default avg 3"),
        make_norm_row("P/B Ratio", "Clamp(100 - (PB / Ceiling) * 100)", "Sector ceiling: 5 asset-heavy / 15 mixed / 40 tech"),
        make_norm_row("EPS Growth", "Clamp(50 + (Growth * 1.5))", "0%->50, +33%->100. EDGAR 10-K realized (not est.)"),
        make_norm_row("Revenue Growth", "Clamp(50 + (Growth * 2))", "0%->50, +25%->100, -25%->0"),
        make_norm_row("Free Cash Flow", "By FCF yield = FCF / Market Cap", "<0%->0-30, 0-5%->30-60, >5%->60-100"),
        make_norm_row("FCF Growth", "Clamp(50 + (Growth * 1.5))", "0%->50, +33%->100 YoY"),
        make_norm_row("Gross Margin", "Clamp((GM / Ceiling) * 100)", "Sector ceiling: 35 retail / 60 mixed / 85 tech"),
        make_norm_row("Operating Margin", "Linear from sector floor to ceiling", "Tech -10..35, retail -5..12, mixed -10..20"),
        make_norm_row("Debt-to-Equity", "Clamp(100 - (D_E / Ceiling) * 100)", "Ceiling: 15 financials/REITs, else 5"),
        make_norm_row("Interest Coverage", "30 + log100(coverage) * 70", "1x->30, 10x->65, 100x->100; <1x scaled 0-30"),
        make_norm_row("Fed Funds Rate", "Piecewise sweet-spot curve", "1-3%->65-90; 0%->50; >6% decays toward 0"),
        make_norm_row("CPI YoY inflation", "100 - |CPI - 2%| * 20", "On-target 2%->100; +/-10 by trend"),
        make_norm_row("News Sentiment", "((Sentiment + 1.0) / 2.0) * 100", "Premium endpoint; falls back to CNN Fear & Greed"),
        make_norm_row("Insider Net", "50 +/- (log10(shares)/6) * 40", "Buy: 50-90, Sell: 10-50 (90-day Form 4)"),
        make_norm_row("Institutional Own.", "Clamp(20 + (Own% * 0.9), 20, 92)", "0%->20, saturates at 92"),
        make_norm_row("Short Interest %", "Clamp(100 - (Short% * 5))", "Lower is better. Short interest >20% -> 0"),
        make_norm_row("Competitor IPOs", "100 - (count * 15), floor 25", "Same-sector IPOs: 0->100, 1->85, 2->70"),
        make_norm_row("Sector Growth", "Clamp(50 + (PeerRevGrowth * 1.5))", "Avg peer revenue-growth proxy (Macro)"),
        make_norm_row("Price Mom (52W)", "Clamp(50 + (52w Change% * 2))", "0%->50, +25%->100 (scored in Sentiment)"),
    ];

    let norm_limit = chunks[2].height.saturating_sub(3) as usize;
    let mut visible_norm = norm_rows.clone();
    if norm_limit < norm_rows.len() && norm_limit > 0 {
        visible_norm.truncate(norm_limit);
    } else if norm_limit == 0 {
        visible_norm.clear();
    }

    let norm_table = Table::new(visible_norm, [
        Constraint::Percentage(20),
        Constraint::Percentage(35),
        Constraint::Percentage(45),
    ])
    .header(Row::new(vec!["Metric", "Normalization Formula", "Logic & Threshold Context"]).style(norm_hdr))
    .block(Block::default().title(" Normalization Rules (Scores scaled 0 - 100) ").borders(Borders::ALL).border_style(Style::default().fg(Color::DarkGray)));
    f.render_widget(norm_table, chunks[2]);
}

fn render_scrollable_panel(
    f: &mut Frame,
    rect: Rect,
    title: &str,
    lines: &[Line],
    border_color: Color,
) {
    let block = Block::default()
        .title(format!(" {} ", title))
        .borders(Borders::ALL)
        .border_style(Style::default().fg(border_color));
        
    let inner_rect = rect.inner(ratatui::layout::Margin { horizontal: 1, vertical: 1 });
    let max_lines = inner_rect.height as usize;
    let max_width = inner_rect.width as usize;
    
    let mut processed_lines = Vec::new();
    for line in lines {
        let mut line_str = String::new();
        for span in &line.spans {
            line_str.push_str(&span.content);
        }
        
        let display_line = if line_str.chars().count() > max_width {
            let truncated = if max_width > 3 {
                format!("{}...", line_str.chars().take(max_width - 3).collect::<String>())
            } else {
                line_str.chars().take(max_width).collect::<String>()
            };
            let style = line.spans.first().map(|s| s.style).unwrap_or_default();
            Line::from(Span::styled(truncated, style))
        } else {
            line.clone()
        };
        processed_lines.push(display_line);
    }
    
    if processed_lines.len() > max_lines && max_lines > 0 {
        processed_lines.truncate(max_lines);
    }
    
    let p = Paragraph::new(processed_lines).block(block);
    f.render_widget(p, rect);
}

fn render_metric_guide(f: &mut Frame, rect: Rect, result: &AnalysisResult) {
    let chunks = Layout::default()
        .direction(Direction::Vertical)
        .constraints([
            Constraint::Length(3),
            Constraint::Min(0),
        ])
        .split(rect);

    let header_block = Block::default()
        .borders(Borders::ALL)
        .border_style(Style::default().fg(Color::Cyan));
    let header_text = format!(" METRIC GUIDE FOR {}: WHY THEY MATTER ", result.horizon.to_string().to_uppercase());
    let header_p = Paragraph::new(Span::styled(header_text, Style::default().fg(Color::Cyan).add_modifier(Modifier::BOLD)))
        .alignment(Alignment::Center)
        .block(header_block);
    f.render_widget(header_p, chunks[0]);

    let body_chunks = Layout::default()
        .direction(Direction::Horizontal)
        .constraints([
            Constraint::Percentage(35),
            Constraint::Percentage(65),
        ])
        .split(chunks[1]);

    let (left_lines, right_lines) = match result.horizon {
        crate::models::Horizon::Short => {
            let left = vec![
                Line::from(Span::styled("CATEGORY WEIGHTS", Style::default().fg(Color::Yellow).add_modifier(Modifier::BOLD))),
                Line::from(Span::raw("• Risk Profile:  35% (HIGH)")),
                Line::from(Span::raw("• Macro Env:     25% (MID-HIGH)")),
                Line::from(Span::raw("• Sentiment:     20% (MID)")),
                Line::from(Span::raw("• Valuation:     10% (LOW)")),
                Line::from(Span::raw("• Fundamentals:  10% (LOW)")),
                Line::from(""),
                Line::from(Span::styled("WHY THIS REGIME?", Style::default().fg(Color::Cyan).add_modifier(Modifier::BOLD))),
                Line::from(Span::raw("Short-term trading is all about")),
                Line::from(Span::raw("capital preservation & catalysts.")),
                Line::from(Span::raw("Fundamentals (like FCF/growth)")),
                Line::from(Span::raw("take quarters to manifest, while")),
                Line::from(Span::raw("risk factors & sentiment dictate")),
                Line::from(Span::raw("price action today.")),
            ];
            
            let right = vec![
                Line::from(Span::styled("CRITICAL SHORT-TERM METRICS:", Style::default().fg(Color::Yellow).add_modifier(Modifier::BOLD))),
                Line::from(""),
                Line::from(Span::styled("1. Short Interest %", Style::default().fg(Color::Cyan).add_modifier(Modifier::BOLD))),
                Line::from(Span::raw("   • High short interest (>15%) creates explosive potential for")),
                Line::from(Span::raw("     short squeezes, or indicates heavy market-wide pessimism.")),
                Line::from(""),
                Line::from(Span::styled("2. Competitor IPOs", Style::default().fg(Color::Cyan).add_modifier(Modifier::BOLD))),
                Line::from(Span::raw("   • A surge of recent IPOs in the same sector dilutes capital")),
                Line::from(Span::raw("     and attention, increasing volatility for established names.")),
                Line::from(""),
                Line::from(Span::styled("3. News Sentiment", Style::default().fg(Color::Cyan).add_modifier(Modifier::BOLD))),
                Line::from(Span::raw("   • Drives daily retail & algorithmic trading volume. Sentiment")),
                Line::from(Span::raw("     shifts trigger immediate buying/selling momentum.")),
                Line::from(""),
                Line::from(Span::styled("4. Insider Net Trading", Style::default().fg(Color::Cyan).add_modifier(Modifier::BOLD))),
                Line::from(Span::raw("   • Direct Form 4 transactions. Executives buying shares is a")),
                Line::from(Span::raw("     highly reliable near-term signal of value or imminent catalysts.")),
                Line::from(""),
                Line::from(Span::styled("5. Macro Environment (Fed Funds & CPI)", Style::default().fg(Color::Cyan).add_modifier(Modifier::BOLD))),
                Line::from(Span::raw("   • Short-term asset prices are heavily tied to interest rate")),
                Line::from(Span::raw("     regimes. Rising rates directly compress short-term multiples.")),
            ];
            (left, right)
        }
        crate::models::Horizon::Medium => {
            let left = vec![
                Line::from(Span::styled("CATEGORY WEIGHTS", Style::default().fg(Color::Yellow).add_modifier(Modifier::BOLD))),
                Line::from(Span::raw("• Valuation:     25% (HIGH)")),
                Line::from(Span::raw("• Fundamentals:  25% (HIGH)")),
                Line::from(Span::raw("• Macro Env:     20% (MID-HIGH)")),
                Line::from(Span::raw("• Sentiment:     15% (MID-LOW)")),
                Line::from(Span::raw("• Risk Profile:  15% (MID-LOW)")),
                Line::from(""),
                Line::from(Span::styled("WHY THIS REGIME?", Style::default().fg(Color::Cyan).add_modifier(Modifier::BOLD))),
                Line::from(Span::raw("Medium-term (weeks to months)")),
                Line::from(Span::raw("investing balanced approach.")),
                Line::from(Span::raw("Stock prices begin to revert to")),
                Line::from(Span::raw("intrinsic value. Financial health")),
                Line::from(Span::raw("and valuation relative to sector")),
                Line::from(Span::raw("become key performance drivers.")),
            ];
            
            let right = vec![
                Line::from(Span::styled("CRITICAL MEDIUM-TERM METRICS:", Style::default().fg(Color::Yellow).add_modifier(Modifier::BOLD))),
                Line::from(""),
                Line::from(Span::styled("1. P/E & P/S vs Sector Averages", Style::default().fg(Color::Cyan).add_modifier(Modifier::BOLD))),
                Line::from(Span::raw("   • Relative valuation. Buying below sector averages offers a")),
                Line::from(Span::raw("     margin of safety as prices catch up or revert to the mean.")),
                Line::from(""),
                Line::from(Span::styled("2. EPS & Revenue Growth YoY", Style::default().fg(Color::Cyan).add_modifier(Modifier::BOLD))),
                Line::from(Span::raw("   • Validates operational momentum. Accelerating growth leads")),
                Line::from(Span::raw("     to post-earnings drift over subsequent weeks/months.")),
                Line::from(""),
                Line::from(Span::styled("3. Free Cash Flow (FCF) & Growth", Style::default().fg(Color::Cyan).add_modifier(Modifier::BOLD))),
                Line::from(Span::raw("   • Verifies quality of earnings. Companies generating strong")),
                Line::from(Span::raw("     FCF can easily weather business cycles without share dilution.")),
                Line::from(""),
                Line::from(Span::styled("4. Sector Growth Score", Style::default().fg(Color::Cyan).add_modifier(Modifier::BOLD))),
                Line::from(Span::raw("   • Sector tailwinds are powerful. Medium-term trends are")),
                Line::from(Span::raw("     frequently driven by sector rotation and thematic trends.")),
                Line::from(""),
                Line::from(Span::styled("5. Debt/Equity & Interest Coverage", Style::default().fg(Color::Cyan).add_modifier(Modifier::BOLD))),
                Line::from(Span::raw("   • High leverage risks becoming a solvency headwind over")),
                Line::from(Span::raw("     several months if interest coverage deteriorates.")),
            ];
            (left, right)
        }
        crate::models::Horizon::Long => {
            let left = vec![
                Line::from(Span::styled("CATEGORY WEIGHTS", Style::default().fg(Color::Yellow).add_modifier(Modifier::BOLD))),
                Line::from(Span::raw("• Fundamentals:  45% (EXTREME)")),
                Line::from(Span::raw("• Valuation:     20% (MID-HIGH)")),
                Line::from(Span::raw("• Macro Env:     15% (MID)")),
                Line::from(Span::raw("• Sentiment:     10% (LOW)")),
                Line::from(Span::raw("• Risk Profile:  10% (LOW)")),
                Line::from(""),
                Line::from(Span::styled("WHY THIS REGIME?", Style::default().fg(Color::Cyan).add_modifier(Modifier::BOLD))),
                Line::from(Span::raw("Over multi-year horizons, stock")),
                Line::from(Span::raw("returns are driven almost entirely")),
                Line::from(Span::raw("by compounding fundamentals.")),
                Line::from(Span::raw("Sentiment, news noise, and short")),
                Line::from(Span::raw("interest fade to zero, while cash")),
                Line::from(Span::raw("generation and debt control win.")),
            ];
            
            let right = vec![
                Line::from(Span::styled("CRITICAL LONG-TERM METRICS:", Style::default().fg(Color::Yellow).add_modifier(Modifier::BOLD))),
                Line::from(""),
                Line::from(Span::styled("1. Long-Term Fundamentals (45% Weight)", Style::default().fg(Color::Cyan).add_modifier(Modifier::BOLD))),
                Line::from(Span::raw("   • EPS & Revenue Growth: The core compounding engine. Consistent")),
                Line::from(Span::raw("     double-digit growth compounds exponentially over years.")),
                Line::from(Span::raw("   • Free Cash Flow (FCF) Growth: The ultimate measure of corporate")),
                Line::from(Span::raw("     wealth. Supports capital expenditure, dividends, and M&A.")),
                Line::from(""),
                Line::from(Span::styled("2. Balance Sheet Quality (Debt-to-Equity)", Style::default().fg(Color::Cyan).add_modifier(Modifier::BOLD))),
                Line::from(Span::raw("   • Over years, economic downturns are guaranteed. Companies with")),
                Line::from(Span::raw("     low debt (D/E < 1.0) and high interest coverage survive and")),
                Line::from(Span::raw("     take market share when highly leveraged peers go bankrupt.")),
                Line::from(""),
                Line::from(Span::styled("3. Valuation Multiples (Margin of Safety)", Style::default().fg(Color::Cyan).add_modifier(Modifier::BOLD))),
                Line::from(Span::raw("   • Overpaying dramatically lowers long-term expected returns.")),
                Line::from(Span::raw("     P/E, P/S, and P/B must be reasonable at entry to ensure compounding.")),
                Line::from(""),
                Line::from(Span::styled("4. Institutional Ownership", Style::default().fg(Color::Cyan).add_modifier(Modifier::BOLD))),
                Line::from(Span::raw("   • High institutional ownership indicates trust by long-term")),
                Line::from(Span::raw("     fiduciaries who stabilize the stock price during downturns.")),
            ];
            (left, right)
        }
    };

    render_scrollable_panel(f, body_chunks[0], "Horizons & Regimes", &left_lines, Color::DarkGray);
    render_scrollable_panel(f, body_chunks[1], "Metric Explanations & Rationale", &right_lines, Color::DarkGray);
}

use ratatui::widgets::Cell;
