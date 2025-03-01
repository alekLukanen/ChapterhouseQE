use std::fs;

use anyhow::Result;

use chapterhouseqe::handlers::query_handler::Status;
use crossterm::event::{self, Event, KeyCode, KeyEvent, KeyEventKind};
use ratatui::{
    layout::{Constraint, Layout, Rect},
    style::{Color, Modifier, Style, Stylize},
    symbols::border,
    text::{Line, Span, Text},
    widgets::{Block, Cell, Gauge, HighlightSpacing, Paragraph, Row, Table, TableState, Wrap},
    DefaultTerminal, Frame,
};
use regex::Regex;

use clap::Parser;

#[derive(Parser, Debug)]
#[command(version, about, long_about = None)]
struct Args {
    /// The logging level (debug, info, warning, error)
    #[arg(short, long)]
    sql_file: String,
}

fn main() -> Result<()> {
    let args = Args::parse();

    let mut terminal = ratatui::init();
    let app_result = QueriesApp::new(args.sql_file).run(&mut terminal);
    ratatui::restore();
    app_result
}

#[derive(Debug)]
struct TableColors {
    header_fg: Color,
    row_fg: Color,
    selected_column_style_fg: Color,
}

impl TableColors {
    const fn new() -> Self {
        Self {
            header_fg: Color::Cyan,
            row_fg: Color::Cyan,
            selected_column_style_fg: Color::Blue,
        }
    }
}

#[derive(Debug)]
struct QueryInfo {
    query_txt: String,
    status: Option<Status>,
}

impl QueryInfo {
    fn terminal(&self) -> bool {
        if let Some(status) = &self.status {
            status.terminal()
        } else {
            false
        }
    }
    fn completed(&self) -> bool {
        match self.status {
            Some(Status::Complete) => true,
            _ => false,
        }
    }
    fn errored(&self) -> bool {
        match self.status {
            Some(Status::Error(_)) => true,
            _ => false,
        }
    }
    fn status_icon(&self) -> &str {
        match self.status {
            Some(Status::Complete) => "✅",
            Some(Status::Error(_)) => "❌",
            Some(Status::Running) => "🔄",
            Some(Status::Queued) => "🕒",
            _ => "🕒",
        }
    }
}

#[derive(Debug)]
pub struct QueriesApp {
    sql_file: String,
    queries: Option<Vec<QueryInfo>>,
    exit: bool,

    table_state: TableState,
    table_colors: TableColors,
}

impl QueriesApp {
    fn new(sql_file: String) -> QueriesApp {
        QueriesApp {
            sql_file,
            queries: None,
            exit: false,
            table_state: TableState::default().with_selected(0),
            table_colors: TableColors::new(),
        }
    }

    pub fn run(&mut self, terminal: &mut DefaultTerminal) -> Result<()> {
        // draw the initial ui
        terminal.draw(|frame| self.draw(frame))?;

        // break up the sql file text in statements
        let sql_data = fs::read_to_string(self.sql_file.clone())?;
        let queries = parse_sql_queries(sql_data)?;

        if queries.len() > 0 {
            self.table_state.select(Some(0));
        }

        self.queries = Some(
            queries
                .iter()
                .map(|item| QueryInfo {
                    query_txt: item.clone(),
                    status: None,
                })
                .collect(),
        );

        while !self.exit {
            terminal.draw(|frame| self.draw(frame))?;
            self.handle_events()?;
        }

        Ok(())
    }

    fn draw(&mut self, frame: &mut Frame) {
        let title = Line::from(vec![Span::styled(
            " Executing Queries ",
            ratatui::style::Style::default().bold().cyan(),
        )]);
        let instructions = Line::from(vec![" Quit ".into(), "<Q> ".blue().bold()]);
        let block = Block::default()
            .title(title.centered())
            .title_bottom(instructions.centered());

        let view_layout =
            Layout::horizontal([Constraint::Percentage(30), Constraint::Percentage(70)]);
        let [left_area, right_area] = view_layout.areas(block.inner(frame.area()));

        let left_layout = Layout::vertical([Constraint::Length(10), Constraint::Fill(1)]);
        let [left_info_area, left_queries_area] = left_layout.areas(left_area);

        self.render_info_area(left_info_area, frame);
        self.render_queries_area(left_queries_area, frame);
        self.render_table_area(right_area, frame);

        frame.render_widget(block, frame.area())
    }

    fn handle_events(&mut self) -> Result<()> {
        match event::read()? {
            // it's important to check that the event is a key press event as
            // crossterm also emits key release and repeat events on Windows.
            Event::Key(key_event) if key_event.kind == KeyEventKind::Press => {
                self.handle_key_event(key_event)
            }
            _ => {}
        };
        Ok(())
    }

    fn handle_key_event(&mut self, key_event: KeyEvent) {
        match key_event.code {
            KeyCode::Char('q') => self.exit(),
            KeyCode::Up => self.previous_row(),
            KeyCode::Down => self.next_row(),
            _ => {}
        }
    }

    fn exit(&mut self) {
        self.exit = true;
    }

    fn render_info_area(&self, area: Rect, frame: &mut Frame) {
        let info_block = Block::bordered()
            .title(" Execution Info ")
            .border_set(border::ROUNDED)
            .border_style(Style::default().cyan());

        if let Some(queries) = &self.queries {
            let queries_completed = queries.iter().filter(|item| item.completed()).count();
            let queries_errored = queries.iter().filter(|item| item.errored()).count();
            let percent_complete = if queries.len() > 0 {
                (queries_completed as f32 / queries.len() as f32) as u16 + 29
            } else {
                0u16
            };

            let total_queries_items = vec![
                Line::from(vec!["File: ".cyan(), format!("{}", self.sql_file).blue()]),
                Line::from(vec![
                    "Total Queries: ".cyan(),
                    format!("{}", queries.len()).blue(),
                ]),
                Line::from(vec![
                    "Completed: ".cyan(),
                    format!("{}", queries_completed).blue(),
                ]),
                Line::from(vec![
                    "Errored: ".cyan(),
                    format!("{}", queries_errored).blue(),
                ]),
            ];
            let query_lines = total_queries_items.len() as u16;

            let total_queries_txt = Paragraph::new(total_queries_items).wrap(Wrap::default());

            let progress_txt = Paragraph::new(Line::from("Progress:".cyan()));
            let progress_bar = Gauge::default()
                .gauge_style(Style::default().blue())
                .percent(percent_complete);

            let info_block_layout = Layout::vertical([
                Constraint::Length(query_lines),
                Constraint::Length(1),
                Constraint::Fill(1),
            ]);
            let [queries_txt_area, progress_txt_area, progress_bar_area] =
                info_block_layout.areas(info_block.inner(area));

            frame.render_widget(total_queries_txt, queries_txt_area);
            frame.render_widget(progress_txt, progress_txt_area);
            frame.render_widget(progress_bar, progress_bar_area);

            frame.render_widget(info_block, area)
        }
    }

    fn render_table_area(&self, area: Rect, frame: &mut Frame) {
        let table_block = Block::bordered()
            .title(Line::from(vec![
                Span::raw(" ✨ "),
                Span::from("Query Data "),
            ]))
            .border_set(border::ROUNDED)
            .border_style(Style::default().fg(Color::Cyan));

        frame.render_widget(table_block, area);
    }

    fn render_queries_area(&mut self, area: Rect, frame: &mut Frame) {
        let queries_block = Block::bordered()
            .title(" Queries ")
            .border_set(border::ROUNDED)
            .border_style(Style::default().cyan());

        let header_style = Style::default().fg(self.table_colors.header_fg);
        let selected_row_style = Style::default()
            .add_modifier(Modifier::REVERSED)
            .fg(self.table_colors.selected_column_style_fg);

        let header = Row::new(vec![
            Cell::default(),
            Cell::from(Text::from("Idx".to_string())),
            Cell::from(Text::from("Query".to_string())),
        ])
        .style(header_style)
        .height(1);

        let rows = if let Some(queries) = &self.queries {
            queries
                .iter()
                .enumerate()
                .map(|(i, data)| {
                    Row::new(vec![
                        Cell::from(
                            Span::from(format!("{}", data.status_icon()))
                                .style(Style::default().fg(Color::Green)),
                        ),
                        Cell::from(Text::from(format!("{}", i)).fg(self.table_colors.row_fg)),
                        Cell::from(
                            Text::from(data.query_txt.to_string()).fg(self.table_colors.row_fg),
                        ),
                    ])
                    .height(4)
                })
                .collect::<Vec<Row>>()
        } else {
            vec![]
        };

        let bar = "█";

        let table = Table::new(
            rows,
            [
                // + 1 is for padding.
                Constraint::Length(2),
                Constraint::Length(5),
                Constraint::Fill(8),
            ],
        )
        .header(header)
        .highlight_style(selected_row_style)
        .highlight_symbol(Text::from(vec![
            "".into(),
            bar.into(),
            bar.into(),
            "".into(),
        ]))
        .highlight_spacing(HighlightSpacing::Always);

        frame.render_stateful_widget(table, queries_block.inner(area), &mut self.table_state);
        frame.render_widget(queries_block, area);
    }

    pub fn next_row(&mut self) {
        let size = if let Some(queries) = &self.queries {
            queries.len()
        } else {
            0
        };
        let i = match self.table_state.selected() {
            Some(i) => {
                if i >= size - 1 {
                    0
                } else {
                    i + 1
                }
            }
            None => 0,
        };
        self.table_state.select(Some(i));
    }

    pub fn previous_row(&mut self) {
        let size = if let Some(queries) = &self.queries {
            queries.len()
        } else {
            0
        };

        let i = match self.table_state.selected() {
            Some(i) => {
                if i == 0 {
                    size - 1
                } else {
                    i - 1
                }
            }
            None => 0,
        };
        self.table_state.select(Some(i));
    }
}

fn parse_sql_queries(val: String) -> Result<Vec<String>> {
    let re = Regex::new(r#"(?s)(?:".*?"|'.*?'|[^'";])*?;"#)?;
    Ok(re
        .find_iter(&val)
        .map(|m| m.as_str().to_string())
        .map(|item| item.trim().to_string())
        .collect())
}
