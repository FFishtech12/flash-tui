use chrono::{Local, NaiveDate, Duration};
use crossterm::{
    event::{self, Event, KeyCode},
    terminal::{disable_raw_mode, enable_raw_mode, EnterAlternateScreen, LeaveAlternateScreen},
    execute,
};
use dirs;
use rand::seq::SliceRandom;
use ratatui::{
    backend::CrosstermBackend,
    layout::{Alignment, Rect},
    style::{Color, Modifier, Style},
    text::{Line, Span},
    widgets::{Block, Borders, List, ListItem, ListState, Paragraph, Wrap},
    Terminal,
};
use serde::{Deserialize, Serialize};
use std::collections::{HashMap, HashSet};
use std::fs;
use std::io;
use std::path::{Path, PathBuf};

// ---------------- Modelo de datos ----------------
#[derive(Debug, Clone, Deserialize)]
pub struct RawCard {
    pub word: String,
    pub translation: String,
    pub meaning: String,
    pub ipa: String,
}

#[derive(Debug, Clone)]
pub struct Card {
    pub language: String,
    pub group: String,
    pub raw: RawCard,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Progress {
    pub interval: u32,
    pub repetitions: u32,
    pub ease_factor: f64,
    pub next_review: NaiveDate,
    pub last_review: Option<NaiveDate>,
}

fn card_id(card: &Card) -> String {
    format!("{}/{}/{}", card.language, card.group, card.raw.word)
}

// ---------------- Almacenamiento de progreso ----------------
pub struct ProgressStore {
    dir: PathBuf,
    data: HashMap<String, Progress>,
}

impl ProgressStore {
    pub fn new() -> Self {
        let mut dir = dirs::data_dir().unwrap_or_else(|| PathBuf::from("."));
        dir.push("flash-tui");
        fs::create_dir_all(&dir).ok();
        let mut store = Self {
            dir,
            data: HashMap::new(),
        };
        store.load();
        store
    }

    fn load(&mut self) {
        if let Ok(entries) = fs::read_dir(&self.dir) {
            for entry in entries.flatten() {
                let path = entry.path();
                if path.is_file()
                    && path
                        .file_name()
                        .and_then(|n| n.to_str())
                        .map_or(false, |n| n.starts_with("progress_") && n.ends_with(".json"))
                {
                    if let Ok(content) = fs::read_to_string(&path) {
                        if let Ok(map) = serde_json::from_str::<HashMap<String, Progress>>(&content) {
                            self.data.extend(map);
                        }
                    }
                }
            }
        }
    }

    pub fn get(&self, id: &str) -> Option<&Progress> {
        self.data.get(id)
    }

    pub fn update(&mut self, id: String, progress: Progress) {
        self.data.insert(id, progress);
    }

    pub fn save(&self) {
        let mut by_lang: HashMap<String, HashMap<String, Progress>> = HashMap::new();
        for (id, prog) in &self.data {
            let lang = id.split('/').next().unwrap_or("unknown").to_string();
            by_lang
                .entry(lang.clone())
                .or_default()
                .insert(id.clone(), prog.clone());
        }
        
        for (lang, map) in &by_lang {
            let file_name = format!("progress_{}.json", lang);
            let path = self.dir.join(file_name);
            if let Ok(json) = serde_json::to_string_pretty(&map) {
                fs::write(path, json).ok();
            }
        }
        
        if let Ok(entries) = fs::read_dir(&self.dir) {
            for entry in entries.flatten() {
                let path = entry.path();
                if let Some(name) = path.file_name().and_then(|n| n.to_str()) {
                    if name.starts_with("progress_") && name.ends_with(".json") {
                        let lang = name
                            .strip_prefix("progress_")
                            .and_then(|s| s.strip_suffix(".json"))
                            .unwrap_or("");
                        if !by_lang.contains_key(lang) {
                            fs::remove_file(path).ok();
                        }
                    }
                }
            }
        }
    }
}

// ---------------- Algoritmo SM-2 ----------------
pub fn sm2_update(progress: &mut Progress, quality: u8, today: NaiveDate) {
    let q = quality as f64;
    let new_ef = (progress.ease_factor + (0.1 - (3.0 - q) * (0.08 + (3.0 - q) * 0.02))).max(1.3);
    progress.ease_factor = new_ef;

    if quality < 2 {
        progress.repetitions = 0;
        progress.interval = 1;
    } else {
        progress.repetitions += 1;
        progress.interval = match progress.repetitions {
            1 => 1,
            2 => 6,
            _ => (progress.interval as f64 * progress.ease_factor).ceil() as u32,
        };
    }
    progress.last_review = Some(today);
    progress.next_review = today + Duration::days(progress.interval as i64);
}

// ---------------- Carga de tarjetas (CON SOPORTE PARA SUBCARPETAS) ----------------
pub fn load_cards(base_dir: &Path) -> Vec<Card> {
    let mut cards = Vec::new();
    if let Ok(entries) = fs::read_dir(base_dir) {
        for entry in entries.flatten() {
            let path = entry.path();
            if path.is_dir() {
                let language = path.file_name().unwrap().to_string_lossy().to_string();
                load_json_files(&path, &language, "", &mut cards);
            }
        }
    }
    cards
}

fn load_json_files(dir: &Path, language: &str, parent_group: &str, cards: &mut Vec<Card>) {
    if let Ok(entries) = fs::read_dir(dir) {
        for entry in entries.flatten() {
            let path = entry.path();
            if path.is_dir() {
                let folder_name = path.file_name().unwrap().to_string_lossy().to_string();
                let new_group = if parent_group.is_empty() {
                    folder_name
                } else {
                    format!("{}/{}", parent_group, folder_name)
                };
                load_json_files(&path, language, &new_group, cards);
            } else if path.extension().map_or(false, |e| e == "json") {
                let file_stem = path.file_stem().unwrap().to_string_lossy().to_string();
                let group = if parent_group.is_empty() {
                    file_stem
                } else {
                    format!("{}/{}", parent_group, file_stem)
                };
                
                if let Ok(content) = fs::read_to_string(&path) {
                    if let Ok(raw_cards) = serde_json::from_str::<Vec<RawCard>>(&content) {
                        for raw in raw_cards {
                            cards.push(Card {
                                language: language.to_string(),
                                group: group.clone(),
                                raw,
                            });
                        }
                    }
                }
            }
        }
    }
}

// ---------------- Estados de la UI ----------------
enum AppState {
    LanguageSelect,
    GroupSelect {
        language: String,
        groups: Vec<String>,
    },
    Study {
        due_cards: Vec<Card>,
        current_index: usize,
        show_back: bool,
        rating_pending: bool,
        cram: bool,
        session_history: Vec<(String, Progress)>,
        ratings_given: Vec<u8>,
    },
    Stats {
        cards_reviewed: usize,
        ratings: Vec<u8>,
        average_quality: f64,
        language: String,
        cram: bool,
    },
    Quit,
}

// ---------------- Filtros ----------------
fn filter_due_cards(all: &[Card], lang: &str, store: &ProgressStore, today: NaiveDate) -> Vec<Card> {
    all.iter()
        .filter(|c| c.language == lang)
        .filter(|c| {
            let id = card_id(c);
            store.get(&id).map_or(true, |p| p.next_review <= today)
        })
        .cloned()
        .collect()
}

fn filter_due_cards_by_group(
    all: &[Card],
    lang: &str,
    group: &str,
    store: &ProgressStore,
    today: NaiveDate,
) -> Vec<Card> {
    all.iter()
        .filter(|c| c.language == lang && c.group == group)
        .filter(|c| {
            let id = card_id(c);
            store.get(&id).map_or(true, |p| p.next_review <= today)
        })
        .cloned()
        .collect()
}

fn collect_cram_cards(all: &[Card], lang: &str) -> Vec<Card> {
    all.iter()
        .filter(|c| c.language == lang)
        .cloned()
        .collect()
}

fn collect_cram_cards_by_group(all: &[Card], lang: &str, group: &str) -> Vec<Card> {
    all.iter()
        .filter(|c| c.language == lang && c.group == group)
        .cloned()
        .collect()
}

fn start_study_session(mut cards: Vec<Card>, cram: bool, app_state: &mut AppState) {
    if cards.is_empty() {
        *app_state = AppState::LanguageSelect;
        return;
    }
    cards.shuffle(&mut rand::thread_rng());
    *app_state = AppState::Study {
        due_cards: cards,
        current_index: 0,
        show_back: false,
        rating_pending: false,
        cram,
        session_history: Vec::new(),
        ratings_given: Vec::new(),
    };
}

// ---------------- UI con HUD ----------------
fn ui(
    f: &mut ratatui::Frame,
    state: &AppState,
    _all_cards: &[Card],
    _store: &ProgressStore,
    _today: &NaiveDate,
    list_state: &mut ListState,
) {
    let area = f.size();
    
    let main_area = Rect {
        x: area.x,
        y: area.y,
        width: area.width,
        height: area.height.saturating_sub(2),
    };
    
    let hud_area = Rect {
        x: area.x,
        y: area.y + area.height.saturating_sub(2),
        width: area.width,
        height: 2,
    };

    match state {
        AppState::LanguageSelect => {
            let mut languages: Vec<String> = _all_cards
                .iter()
                .map(|c| c.language.clone())
                .collect::<HashSet<_>>()
                .into_iter()
                .collect();
            languages.sort();
            
            let items: Vec<ListItem> = languages
                .iter()
                .map(|l| ListItem::new(l.as_str()))
                .collect();
            let list = List::new(items)
                .block(Block::default()
                    .title("🌍 Selecciona un idioma")
                    .borders(Borders::ALL))
                .highlight_style(Style::default()
                    .bg(Color::DarkGray)
                    .add_modifier(Modifier::BOLD));
            f.render_stateful_widget(list, main_area, list_state);
            
            let hud_text = vec![
                Line::from(vec![
                    Span::styled(" ↑↓ ", Style::default().fg(Color::Yellow).add_modifier(Modifier::BOLD)),
                    Span::raw("Navegar  "),
                    Span::styled(" Enter ", Style::default().fg(Color::Green).add_modifier(Modifier::BOLD)),
                    Span::raw("Seleccionar  "),
                    Span::styled(" Q/Esc ", Style::default().fg(Color::Red).add_modifier(Modifier::BOLD)),
                    Span::raw("Salir"),
                ]),
            ];
            let hud = Paragraph::new(hud_text)
                .block(Block::default().borders(Borders::ALL))
                .style(Style::default().fg(Color::White));
            f.render_widget(hud, hud_area);
        }
        
        AppState::GroupSelect { language, groups } => {
            let title = format!("📚 {} - Selecciona grupo", language);
            let items: Vec<ListItem> = groups
                .iter()
                .map(|g| {
                    if g == "Todos los grupos" {
                        ListItem::new(format!("🌟 {}", g))
                    } else {
                        ListItem::new(format!("📁 {}", g))
                    }
                })
                .collect();
            let list = List::new(items)
                .block(Block::default().title(title).borders(Borders::ALL))
                .highlight_style(Style::default()
                    .bg(Color::DarkGray)
                    .add_modifier(Modifier::BOLD));
            f.render_stateful_widget(list, main_area, list_state);
            
            let hud_text = vec![
                Line::from(vec![
                    Span::styled(" ↑↓ ", Style::default().fg(Color::Yellow).add_modifier(Modifier::BOLD)),
                    Span::raw("Navegar  "),
                    Span::styled(" Enter ", Style::default().fg(Color::Green).add_modifier(Modifier::BOLD)),
                    Span::raw("Repaso  "),
                    Span::styled(" C ", Style::default().fg(Color::Cyan).add_modifier(Modifier::BOLD)),
                    Span::raw("Cram  "),
                    Span::styled(" Esc ", Style::default().fg(Color::Magenta).add_modifier(Modifier::BOLD)),
                    Span::raw("Volver  "),
                    Span::styled(" Q ", Style::default().fg(Color::Red).add_modifier(Modifier::BOLD)),
                    Span::raw("Salir"),
                ]),
            ];
            let hud = Paragraph::new(hud_text)
                .block(Block::default().borders(Borders::ALL))
                .style(Style::default().fg(Color::White));
            f.render_widget(hud, hud_area);
        }
        
        AppState::Study {
            due_cards,
            current_index,
            show_back,
            rating_pending,
            cram,
            ..
        } => {
            if due_cards.is_empty() {
                let p = Paragraph::new("🎉 No hay tarjetas para repasar hoy\n\nPresiona Esc para volver")
                    .block(Block::default().borders(Borders::ALL))
                    .alignment(Alignment::Center);
                f.render_widget(p, main_area);
                
                let hud_text = vec![
                    Line::from(vec![
                        Span::styled(" Esc ", Style::default().fg(Color::Magenta).add_modifier(Modifier::BOLD)),
                        Span::raw("Volver a idiomas  "),
                        Span::styled(" Q ", Style::default().fg(Color::Red).add_modifier(Modifier::BOLD)),
                        Span::raw("Salir"),
                    ]),
                ];
                let hud = Paragraph::new(hud_text)
                    .block(Block::default().borders(Borders::ALL))
                    .style(Style::default().fg(Color::White));
                f.render_widget(hud, hud_area);
                return;
            }
            
            let card = &due_cards[*current_index];
            let mut lines = vec![
                Line::from(vec![
                    Span::styled("📁 Grupo: ", Style::default().fg(Color::Yellow).add_modifier(Modifier::BOLD)),
                    Span::raw(&card.group),
                ]),
                Line::from(""),
                Line::from(vec![
                    Span::styled("🔤 Palabra: ", Style::default().fg(Color::Cyan).add_modifier(Modifier::BOLD)),
                    Span::raw(&card.raw.word),
                ]),
                Line::from(vec![
                    Span::styled("🔊 IPA: ", Style::default().fg(Color::Magenta).add_modifier(Modifier::BOLD)),
                    Span::raw(&card.raw.ipa),
                ]),
            ];

            if *show_back {
                lines.push(Line::from("─".repeat(40)));
                lines.push(Line::from(vec![
                    Span::styled("🌍 Traducción: ", Style::default().fg(Color::Green).add_modifier(Modifier::BOLD)),
                    Span::raw(&card.raw.translation),
                ]));
                lines.push(Line::from(vec![
                    Span::styled("📖 Significado: ", Style::default().fg(Color::Green).add_modifier(Modifier::BOLD)),
                    Span::raw(&card.raw.meaning),
                ]));
                if *rating_pending {
                    lines.push(Line::from(""));
                    lines.push(Line::from("─".repeat(40)));
                    lines.push(Line::from(vec![
                        Span::styled("❌ 0:Again ", Style::default().fg(Color::Red).add_modifier(Modifier::BOLD)),
                        Span::styled("😅 1:Hard ", Style::default().fg(Color::LightRed).add_modifier(Modifier::BOLD)),
                        Span::styled("✅ 2:Good ", Style::default().fg(Color::LightGreen).add_modifier(Modifier::BOLD)),
                        Span::styled("🚀 3:Easy", Style::default().fg(Color::Green).add_modifier(Modifier::BOLD)),
                    ]));
                }
            } else {
                lines.push(Line::from(""));
                lines.push(Line::from("Presiona ESPACIO para ver la respuesta"));
            }
            if *current_index > 0 {
                lines.push(Line::from(""));
                lines.push(Line::from("💡 Presiona B para volver a la tarjeta anterior"));
            }

            let mode = if *cram { "⚡ CRAM" } else { "🔄 REPASO" };
            let title = format!(" {} {}/{} [{}] ", 
                card.language, current_index + 1, due_cards.len(), mode);
            let p = Paragraph::new(lines)
                .block(Block::default().borders(Borders::ALL).title(title))
                .wrap(Wrap { trim: true });
            f.render_widget(p, main_area);
            
            let hud_text = if *rating_pending {
                vec![
                    Line::from(vec![
                        Span::styled(" 0-3 ", Style::default().fg(Color::Green).add_modifier(Modifier::BOLD)),
                        Span::raw("Calificar  "),
                        Span::styled(" B ", Style::default().fg(Color::Yellow).add_modifier(Modifier::BOLD)),
                        Span::raw("Atrás  "),
                        Span::styled(" Esc ", Style::default().fg(Color::Magenta).add_modifier(Modifier::BOLD)),
                        Span::raw("Volver  "),
                        Span::styled(" Q ", Style::default().fg(Color::Red).add_modifier(Modifier::BOLD)),
                        Span::raw("Salir"),
                    ]),
                ]
            } else {
                vec![
                    Line::from(vec![
                        Span::styled(" Espacio ", Style::default().fg(Color::Green).add_modifier(Modifier::BOLD)),
                        Span::raw("Ver respuesta  "),
                        Span::styled(" B ", Style::default().fg(Color::Yellow).add_modifier(Modifier::BOLD)),
                        Span::raw("Atrás  "),
                        Span::styled(" Esc ", Style::default().fg(Color::Magenta).add_modifier(Modifier::BOLD)),
                        Span::raw("Volver  "),
                        Span::styled(" Q ", Style::default().fg(Color::Red).add_modifier(Modifier::BOLD)),
                        Span::raw("Salir"),
                    ]),
                ]
            };
            let hud = Paragraph::new(hud_text)
                .block(Block::default().borders(Borders::ALL))
                .style(Style::default().fg(Color::White));
            f.render_widget(hud, hud_area);
        }
        
        AppState::Stats {
            cards_reviewed,
            ratings,
            average_quality,
            language,
            cram,
        } => {
            let mut count = [0u32; 4];
            for &q in ratings {
                if q < 4 {
                    count[q as usize] += 1;
                }
            }
            let lines = vec![
                Line::from(vec![Span::styled(
                    "🎉 ¡Sesión completada!",
                    Style::default().fg(Color::Green).add_modifier(Modifier::BOLD),
                )]),
                Line::from(""),
                Line::from(format!("Idioma: {}", language)),
                Line::from(format!("Modo: {}", if *cram { "⚡ Cram" } else { "🔄 Repaso espaciado" })),
                Line::from(format!("Tarjetas repasadas: {}", cards_reviewed)),
                Line::from(format!("Calidad promedio: {:.2}/3.0", average_quality)),
                Line::from(""),
                Line::from("Distribución de respuestas:"),
                Line::from(format!("  ❌ Again (0): {}", count[0])),
                Line::from(format!("  😅 Hard  (1): {}", count[1])),
                Line::from(format!("  ✅ Good  (2): {}", count[2])),
                Line::from(format!("  🚀 Easy  (3): {}", count[3])),
                Line::from(""),
                Line::from("Presiona cualquier tecla para volver al menú"),
            ];
            let p = Paragraph::new(lines)
                .block(Block::default().borders(Borders::ALL).title("📊 Estadísticas"))
                .alignment(Alignment::Center);
            f.render_widget(p, main_area);
            
            let hud_text = vec![
                Line::from(vec![
                    Span::styled(" Cualquier tecla ", Style::default().fg(Color::Green).add_modifier(Modifier::BOLD)),
                    Span::raw("Volver al menú  "),
                    Span::styled(" Q ", Style::default().fg(Color::Red).add_modifier(Modifier::BOLD)),
                    Span::raw("Salir"),
                ]),
            ];
            let hud = Paragraph::new(hud_text)
                .block(Block::default().borders(Borders::ALL))
                .style(Style::default().fg(Color::White));
            f.render_widget(hud, hud_area);
        }
        
        AppState::Quit => {}
    }
}

// ---------------- Main ----------------
fn main() -> Result<(), Box<dyn std::error::Error>> {
    enable_raw_mode()?;
    let mut stdout = io::stdout();
    execute!(stdout, EnterAlternateScreen)?;
    let backend = CrosstermBackend::new(stdout);
    let mut terminal = Terminal::new(backend)?;

    let base_dir = std::env::current_dir()?;
    let all_cards = load_cards(&base_dir);
    let mut progress_store = ProgressStore::new();
    let today = Local::now().date_naive();

    let mut languages: Vec<String> = all_cards
        .iter()
        .map(|c| c.language.clone())
        .collect::<HashSet<_>>()
        .into_iter()
        .collect();
    languages.sort();

    let mut app_state = AppState::LanguageSelect;
    let mut list_state = ListState::default();
    list_state.select(Some(0));

    loop {
        terminal.draw(|f| {
            ui(
                f,
                &app_state,
                &all_cards,
                &progress_store,
                &today,
                &mut list_state,
            )
        })?;

        if let Event::Key(key) = event::read()? {
            if key.code == KeyCode::Char('q') {
                app_state = AppState::Quit;
                break;
            }

            match &mut app_state {
                AppState::Quit => break,
                
                AppState::LanguageSelect => {
                    match key.code {
                        KeyCode::Esc => {
                            app_state = AppState::Quit;
                            break;
                        }
                        KeyCode::Enter => {
                            if let Some(i) = list_state.selected() {
                                if i < languages.len() {
                                    let language = languages[i].clone();
                                    let mut groups: Vec<String> = all_cards
                                        .iter()
                                        .filter(|c| c.language == language)
                                        .map(|c| c.group.clone())
                                        .collect::<HashSet<_>>()
                                        .into_iter()
                                        .collect();
                                    groups.sort();
                                    let mut options = vec!["Todos los grupos".to_string()];
                                    options.extend(groups);
                                    app_state = AppState::GroupSelect {
                                        language,
                                        groups: options,
                                    };
                                    list_state.select(Some(0));
                                }
                            }
                        }
                        KeyCode::Up => {
                            if let Some(sel) = list_state.selected() {
                                if sel > 0 {
                                    list_state.select(Some(sel - 1));
                                }
                            }
                        }
                        KeyCode::Down => {
                            if let Some(sel) = list_state.selected() {
                                if sel + 1 < languages.len() {
                                    list_state.select(Some(sel + 1));
                                }
                            }
                        }
                        _ => {}
                    }
                }
                
                AppState::GroupSelect { language, groups } => {
                    match key.code {
                        KeyCode::Esc => {
                            app_state = AppState::LanguageSelect;
                            list_state.select(Some(0));
                        }
                        KeyCode::Enter => {
                            if let Some(i) = list_state.selected() {
                                if i < groups.len() {
                                    let selected = groups[i].clone();
                                    let cards = if selected == "Todos los grupos" {
                                        filter_due_cards(&all_cards, language, &progress_store, today)
                                    } else {
                                        filter_due_cards_by_group(
                                            &all_cards,
                                            language,
                                            &selected,
                                            &progress_store,
                                            today,
                                        )
                                    };
                                    start_study_session(cards, false, &mut app_state);
                                }
                            }
                        }
                        KeyCode::Char('c') => {
                            if let Some(i) = list_state.selected() {
                                if i < groups.len() {
                                    let selected = groups[i].clone();
                                    let cards = if selected == "Todos los grupos" {
                                        collect_cram_cards(&all_cards, language)
                                    } else {
                                        collect_cram_cards_by_group(&all_cards, language, &selected)
                                    };
                                    start_study_session(cards, true, &mut app_state);
                                }
                            }
                        }
                        KeyCode::Up => {
                            if let Some(sel) = list_state.selected() {
                                if sel > 0 {
                                    list_state.select(Some(sel - 1));
                                }
                            }
                        }
                        KeyCode::Down => {
                            if let Some(sel) = list_state.selected() {
                                if sel + 1 < groups.len() {
                                    list_state.select(Some(sel + 1));
                                }
                            }
                        }
                        _ => {}
                    }
                }
                
                AppState::Study {
                    due_cards,
                    current_index,
                    show_back,
                    rating_pending,
                    session_history,
                    ratings_given,
                    cram,
                } => {
                    match key.code {
                        KeyCode::Esc => {
                            app_state = AppState::LanguageSelect;
                            list_state.select(Some(0));
                        }
                        KeyCode::Char(' ') => {
                            if !*rating_pending {
                                *show_back = true;
                                *rating_pending = true;
                            }
                        }
                        KeyCode::Char(c @ '0'..='3') if *rating_pending => {
                            let quality = c.to_digit(10).unwrap() as u8;
                            let card = &due_cards[*current_index];
                            let id = card_id(card);
                            let old_progress = progress_store
                                .get(&id)
                                .cloned()
                                .unwrap_or(Progress {
                                    interval: 1,
                                    repetitions: 0,
                                    ease_factor: 2.5,
                                    next_review: today,
                                    last_review: None,
                                });
                            let mut new_progress = old_progress.clone();
                            sm2_update(&mut new_progress, quality, today);
                            progress_store.update(id.clone(), new_progress);

                            session_history.push((id, old_progress));
                            ratings_given.push(quality);

                            if *current_index + 1 < due_cards.len() {
                                *current_index += 1;
                                *show_back = false;
                                *rating_pending = false;
                            } else {
                                let language = due_cards
                                    .first()
                                    .map(|c| c.language.clone())
                                    .unwrap_or_default();
                                let stats = AppState::Stats {
                                    cards_reviewed: ratings_given.len(),
                                    ratings: ratings_given.clone(),
                                    average_quality: ratings_given.iter().sum::<u8>() as f64
                                        / ratings_given.len() as f64,
                                    language,
                                    cram: *cram,
                                };
                                app_state = stats;
                            }
                        }
                        KeyCode::Char('b') if *current_index > 0 => {
                            if let Some((card_id, old_progress)) = session_history.pop() {
                                progress_store.update(card_id, old_progress);
                                ratings_given.pop();
                                *current_index -= 1;
                                *show_back = true;
                                *rating_pending = true;
                            }
                        }
                        _ => {}
                    }
                }
                
                AppState::Stats { .. } => {
                    app_state = AppState::LanguageSelect;
                    list_state.select(Some(0));
                }
            }
        }
    }

    disable_raw_mode()?;
    execute!(terminal.backend_mut(), LeaveAlternateScreen)?;
    progress_store.save();
    Ok(())
}
