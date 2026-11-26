//! Settings: providers and models, workspace rules (prompt layer 1), budgets, execution,
//! prices, and the app's own repo / updates.
//! Design: design/screens/settings.html. Everything edits a local copy of the config; a
//! sticky bar offers Save / Revert while it differs from the engine's config.
//! Sources, triggers, notifications, policies and memory from the mockup are not part of
//! this release.

use std::collections::{BTreeMap, HashMap, HashSet};
use std::path::PathBuf;
use std::time::Instant;

use gpui::{Animation, AnimationExt as _, ClipboardItem, PathPromptOptions};
use gpui_component::input::InputState;
use gpui_component::select::SelectState;
use gpui_component::tooltip::Tooltip;
use harness_core::config::Price;
use harness_core::config::Provider;
use harness_core::skills::Skill;
use harness_engine::update::UpdateStatus;
use harness_engine::{templates, update};

use super::prelude::*;
use crate::ui::input::{self, InputEvent};
use crate::ui::select::{self as sel, Options, SelectEvent, SelectOption};

type Errors = HashMap<String, String>;

// ---------------------------------------------------------------------------
// Sections
// ---------------------------------------------------------------------------

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
enum Section {
    Models,
    Rules,
    Budgets,
    Execution,
    Prices,
    App,
}

impl Section {
    const ALL: [Section; 6] = [
        Section::Models,
        Section::Rules,
        Section::Budgets,
        Section::Execution,
        Section::Prices,
        Section::App,
    ];

    fn label(self) -> &'static str {
        match self {
            Section::Models => "Models & providers",
            Section::Rules => "Workspace rules",
            Section::Budgets => "Budgets",
            Section::Execution => "Execution",
            Section::Prices => "Prices",
            Section::App => "App & updates",
        }
    }
    fn icon(self) -> AppIcon {
        match self {
            Section::Models => AppIcon::Bot,
            Section::Rules => AppIcon::File,
            Section::Budgets => AppIcon::Activity,
            Section::Execution => AppIcon::Terminal,
            Section::Prices => AppIcon::Sliders,
            Section::App => AppIcon::Download,
        }
    }
}

// ---------------------------------------------------------------------------
// Form state
// ---------------------------------------------------------------------------

struct ProviderRow {
    uid: usize,
    kind: String,
    id: Entity<InputState>,
    url: Entity<InputState>,
    key: Entity<InputState>,
    env: Entity<InputState>,
    show_key: bool,
}

struct PriceRow {
    uid: usize,
    family: Entity<InputState>,
    input: Entity<InputState>,
    output: Entity<InputState>,
}

/// All editable widgets, built from a [`Config`]. Replaced wholesale on revert / reload.
struct Form {
    providers: Vec<ProviderRow>,
    models: Vec<String>,
    default_model: String,
    fast_model: String,
    default_sel: Entity<SelectState<Options>>,
    fast_sel: Entity<SelectState<Options>>,
    model_input: Entity<InputState>,
    rules: Entity<InputState>,
    daily: Entity<InputState>,
    monthly: Entity<InputState>,
    slots: Entity<InputState>,
    render_cmd: Entity<InputState>,
    self_repo: Entity<InputState>,
    skill_dirs: Vec<(usize, Entity<InputState>)>,
    prices: Vec<PriceRow>,
    next_uid: usize,
    _subs: Vec<Subscription>,
}

/// A single-line input whose edits re-validate the draft.
fn make_input(
    subs: &mut Vec<Subscription>,
    placeholder: &str,
    value: impl Into<SharedString>,
    masked: bool,
    window: &mut Window,
    cx: &mut Context<SettingsScreen>,
) -> Entity<InputState> {
    let placeholder = SharedString::from(placeholder.to_string());
    let value = value.into();
    let state = cx.new(|cx| {
        let s = InputState::new(window, cx)
            .placeholder(placeholder)
            .default_value(value);
        if masked {
            s.masked(true)
        } else {
            s
        }
    });
    subs.push(
        cx.subscribe_in(&state, window, |this, _, ev: &InputEvent, _, cx| {
            if matches!(ev, InputEvent::Change) {
                this.recompute(cx);
            }
        }),
    );
    state
}

fn num(v: f64) -> String {
    v.to_string()
}

impl Form {
    fn new(
        cfg: &Config,
        detected_repo: &str,
        window: &mut Window,
        cx: &mut Context<SettingsScreen>,
    ) -> Form {
        let mut subs = Vec::new();
        let mut next_uid = 0;
        let mut uid = || {
            next_uid += 1;
            next_uid
        };
        let providers = cfg
            .providers
            .iter()
            .map(|p| provider_row(uid(), p, &mut subs, window, cx))
            .collect();
        let skill_dirs = cfg
            .skill_dirs
            .iter()
            .map(|d| {
                (
                    uid(),
                    make_input(&mut subs, "~/skills", d.clone(), false, window, cx),
                )
            })
            .collect();
        let prices = cfg
            .prices
            .iter()
            .map(|(k, p)| price_row(uid(), k, p, &mut subs, window, cx))
            .collect();

        let rules = input::text_area(
            "One rule per line: language, voice, house rules.",
            14,
            window,
            cx,
        );
        rules.update(cx, |s, cx| s.set_value(cfg.rules.clone(), window, cx));
        subs.push(
            cx.subscribe_in(&rules, window, |this, _, ev: &InputEvent, _, cx| {
                if matches!(ev, InputEvent::Change) {
                    this.recompute(cx);
                }
            }),
        );

        let default_sel = sel::select_state(vec![], None, true, window, cx);
        let fast_sel = sel::select_state(vec![], None, true, window, cx);
        subs.push(cx.subscribe_in(
            &default_sel,
            window,
            |this, _, ev: &SelectEvent<Options>, _, cx| {
                let SelectEvent::Confirm(v) = ev;
                if let Some(v) = v {
                    this.form.default_model = v.clone();
                    this.recompute(cx);
                }
            },
        ));
        subs.push(cx.subscribe_in(
            &fast_sel,
            window,
            |this, _, ev: &SelectEvent<Options>, _, cx| {
                let SelectEvent::Confirm(v) = ev;
                if let Some(v) = v {
                    this.form.fast_model = v.clone();
                    this.recompute(cx);
                }
            },
        ));

        let model_input = make_input(
            &mut subs,
            "provider/model, e.g. nr/cc/claude-sonnet-5-5",
            "",
            false,
            window,
            cx,
        );
        let daily = make_input(&mut subs, "0", num(cfg.daily_budget_usd), false, window, cx);
        let monthly = make_input(
            &mut subs,
            "0",
            num(cfg.monthly_budget_usd),
            false,
            window,
            cx,
        );
        let slots = make_input(
            &mut subs,
            "6",
            cfg.parallel_slots.to_string(),
            false,
            window,
            cx,
        );
        let render_cmd = make_input(
            &mut subs,
            "empty = chromium from PATH",
            cfg.render_command.clone(),
            false,
            window,
            cx,
        );
        let repo_placeholder = if detected_repo.is_empty() {
            "not detected"
        } else {
            detected_repo
        };
        let self_repo = make_input(
            &mut subs,
            repo_placeholder,
            cfg.self_repo.clone(),
            false,
            window,
            cx,
        );

        let form = Form {
            providers,
            models: cfg.models.clone(),
            default_model: cfg.default_model.clone(),
            fast_model: cfg.fast_model.clone(),
            default_sel,
            fast_sel,
            model_input,
            rules,
            daily,
            monthly,
            slots,
            render_cmd,
            self_repo,
            skill_dirs,
            prices,
            next_uid,
            _subs: subs,
        };
        form.refresh_selects(window, cx);
        form
    }

    fn uid(&mut self) -> usize {
        self.next_uid += 1;
        self.next_uid
    }

    /// Every model id that can be picked: the list plus the current default and fast.
    fn choices(&self) -> Vec<String> {
        let mut v: Vec<String> = Vec::new();
        for m in self
            .models
            .iter()
            .chain([&self.default_model, &self.fast_model])
        {
            if !m.is_empty() && !v.contains(m) {
                v.push(m.clone());
            }
        }
        v
    }

    /// Rebuilds the two model selects from the current list.
    fn refresh_selects(&self, window: &mut Window, cx: &mut Context<SettingsScreen>) {
        let choices = self.choices();
        let opts = choices
            .iter()
            .map(|m| SelectOption::plain(m.clone()))
            .collect();
        sel::set_options(
            &self.default_sel,
            opts,
            Some(&self.default_model),
            window,
            cx,
        );
        let mut fast: Vec<SelectOption> = vec![SelectOption::new("", "Same as default")];
        fast.extend(choices.iter().map(|m| SelectOption::plain(m.clone())));
        sel::set_options(&self.fast_sel, fast, Some(&self.fast_model), window, cx);
    }
}

fn provider_row(
    uid: usize,
    p: &Provider,
    subs: &mut Vec<Subscription>,
    window: &mut Window,
    cx: &mut Context<SettingsScreen>,
) -> ProviderRow {
    ProviderRow {
        uid,
        kind: p.kind.clone(),
        id: make_input(subs, "nr", p.id.clone(), false, window, cx),
        url: make_input(
            subs,
            "https://api.example.com/v1",
            p.base_url.clone(),
            false,
            window,
            cx,
        ),
        key: make_input(subs, "sk-…", p.api_key.clone(), true, window, cx),
        env: make_input(
            subs,
            "PROVIDER_API_KEY",
            p.api_key_env.clone(),
            false,
            window,
            cx,
        ),
        show_key: false,
    }
}

fn price_row(
    uid: usize,
    family: &str,
    p: &Price,
    subs: &mut Vec<Subscription>,
    window: &mut Window,
    cx: &mut Context<SettingsScreen>,
) -> PriceRow {
    let blank = family.is_empty();
    PriceRow {
        uid,
        family: make_input(
            subs,
            "model family, e.g. opus",
            family.to_string(),
            false,
            window,
            cx,
        ),
        input: make_input(
            subs,
            "0",
            if blank { String::new() } else { num(p.input) },
            false,
            window,
            cx,
        ),
        output: make_input(
            subs,
            "0",
            if blank { String::new() } else { num(p.output) },
            false,
            window,
            cx,
        ),
    }
}

// ---------------------------------------------------------------------------
// Test / update state
// ---------------------------------------------------------------------------

enum TestState {
    Running,
    Ok { ms: u128, reply: String },
    Err(String),
}

enum UpdateMsg {
    Line(String),
    Done(Result<PathBuf, String>),
}

#[derive(Default)]
struct UpdateUi {
    status: Option<Result<UpdateStatus, String>>,
    loading: bool,
    requested: bool,
    running: bool,
    log: Vec<String>,
}

// ---------------------------------------------------------------------------
// Screen
// ---------------------------------------------------------------------------

pub struct SettingsScreen {
    /// The engine's config as last seen.
    saved: Config,
    /// The config the form currently describes (invalid fields fall back to saved values).
    draft: Config,
    errors: Errors,
    form: Form,
    section: Section,
    tests: HashMap<String, TestState>,
    skills: Vec<Skill>,
    upd: UpdateUi,
    detected_repo: String,
    v1_available: bool,
    saving: bool,
    _subs: Vec<Subscription>,
}

impl SettingsScreen {
    pub fn new(window: &mut Window, cx: &mut Context<Self>) -> Self {
        let app = AppState::global(cx);
        let sub = cx.subscribe_in(&app, window, |this, _, ev: &AppEvent, window, cx| {
            if let AppEvent::StoreChanged { changes, .. } = ev {
                if changes
                    .iter()
                    .any(|c| matches!(c, Change::Config | Change::External))
                {
                    this.config_changed(window, cx);
                }
            }
        });
        let saved = state::engine(cx).config();
        let detected_repo = templates::self_repo(&Config::default())
            .map(|p| p.display().to_string())
            .unwrap_or_default();
        let form = Form::new(&saved, &detected_repo, window, cx);
        let v1_available = std::env::var_os("HOME")
            .map(|h| {
                PathBuf::from(h)
                    .join(".config/harness/config.toml")
                    .exists()
            })
            .unwrap_or(false);
        let mut this = SettingsScreen {
            draft: saved.clone(),
            saved,
            errors: Errors::new(),
            form,
            section: Section::Models,
            tests: HashMap::new(),
            skills: vec![],
            upd: UpdateUi::default(),
            detected_repo,
            v1_available,
            saving: false,
            _subs: vec![sub],
        };
        this.recompute(cx);
        this.load_skills(cx);
        this
    }

    // ---- draft ---------------------------------------------------------------

    fn dirty(&self) -> bool {
        self.draft != self.saved
    }

    /// Re-reads every widget into `draft` and re-validates.
    fn recompute(&mut self, cx: &mut Context<Self>) {
        let (draft, errors) = self.collect(cx);
        self.draft = draft;
        self.errors = errors;
        cx.notify();
    }

    fn collect(&self, cx: &App) -> (Config, Errors) {
        let f = &self.form;
        let mut c = self.saved.clone();
        let mut err = Errors::new();
        let text = |e: &Entity<InputState>| e.read(cx).value().trim().to_string();

        // providers
        let mut seen = HashSet::new();
        c.providers = f
            .providers
            .iter()
            .enumerate()
            .map(|(ix, r)| {
                let id = text(&r.id);
                if id.is_empty() {
                    err.insert(format!("prov.{ix}.id"), "Required".into());
                } else if id.contains('/') || id.contains(char::is_whitespace) {
                    err.insert(format!("prov.{ix}.id"), "No slashes or spaces".into());
                } else if !seen.insert(id.clone()) {
                    err.insert(
                        format!("prov.{ix}.id"),
                        "Another provider uses this id".into(),
                    );
                }
                let url = text(&r.url);
                if let Some(e) = url_error(&url) {
                    err.insert(format!("prov.{ix}.url"), e);
                }
                Provider {
                    id,
                    kind: r.kind.clone(),
                    base_url: url,
                    api_key: r.key.read(cx).unmask_value().trim().to_string(),
                    api_key_env: text(&r.env),
                }
            })
            .collect();
        c.models = f.models.clone();
        c.default_model = f.default_model.clone();
        c.fast_model = f.fast_model.clone();

        // rules
        c.rules = f.rules.read(cx).value().to_string();

        // budgets
        c.daily_budget_usd = parse_money(&text(&f.daily), "budget.daily", &mut err)
            .unwrap_or(self.saved.daily_budget_usd);
        c.monthly_budget_usd = parse_money(&text(&f.monthly), "budget.monthly", &mut err)
            .unwrap_or(self.saved.monthly_budget_usd);

        // execution
        match text(&f.slots).parse::<u32>() {
            Ok(n) if (1..=64).contains(&n) => c.parallel_slots = n,
            _ => {
                err.insert("slots".into(), "A whole number from 1 to 64".into());
            }
        }
        let cmd = text(&f.render_cmd);
        if !cmd.is_empty() && !(cmd.contains("{in}") && cmd.contains("{out}")) {
            err.insert("render".into(), "Must contain {in} and {out}".into());
        }
        c.render_command = cmd;
        c.skill_dirs = f
            .skill_dirs
            .iter()
            .map(|(_, e)| text(e))
            .filter(|d| !d.is_empty())
            .collect();
        c.self_repo = text(&f.self_repo);

        // prices
        let mut prices = BTreeMap::new();
        for (ix, r) in f.prices.iter().enumerate() {
            let family = text(&r.family).to_lowercase();
            if family.is_empty() {
                err.insert(format!("price.{ix}.family"), "Required".into());
                continue;
            }
            if prices.contains_key(&family) {
                err.insert(format!("price.{ix}.family"), "Listed twice".into());
            }
            let input =
                parse_money(&text(&r.input), &format!("price.{ix}.input"), &mut err).unwrap_or(0.0);
            let output = parse_money(&text(&r.output), &format!("price.{ix}.output"), &mut err)
                .unwrap_or(0.0);
            prices.insert(family, Price { input, output });
        }
        c.prices = prices;
        (c, err)
    }

    fn rebuild(&mut self, cfg: Config, window: &mut Window, cx: &mut Context<Self>) {
        self.saved = cfg;
        self.form = Form::new(&self.saved, &self.detected_repo, window, cx);
        self.recompute(cx);
    }

    fn config_changed(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let new = state::engine(cx).config();
        if new == self.saved {
            return;
        }
        let was_dirty = self.dirty() || !self.errors.is_empty();
        if self.draft == new {
            // Our own save landed: keep the widgets (and focus), just move the baseline.
            self.saved = new;
            self.recompute(cx);
        } else if !was_dirty {
            self.rebuild(new, window, cx);
        } else {
            self.saved = new;
            self.recompute(cx);
        }
        self.load_skills(cx);
    }

    fn revert(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let saved = self.saved.clone();
        self.rebuild(saved, window, cx);
    }

    fn save(&mut self, cx: &mut Context<Self>) {
        self.recompute(cx);
        if !self.errors.is_empty() || self.saving {
            return;
        }
        let cfg = self.draft.clone();
        self.saving = true;
        let job = state::run_engine(cx, move |e| e.set_config(cfg));
        cx.spawn(async move |this, cx| {
            let ok = job.await.is_some();
            let _ = this.update(cx, |this, cx| {
                this.saving = false;
                if ok {
                    state::toast(ToastKind::Success, "Settings saved", cx);
                }
                cx.notify();
            });
        })
        .detach();
    }

    // ---- models ----------------------------------------------------------------

    fn add_provider(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let p = Provider {
            id: String::new(),
            kind: "openai".into(),
            base_url: String::new(),
            api_key: String::new(),
            api_key_env: String::new(),
        };
        let uid = self.form.uid();
        let row = provider_row(uid, &p, &mut self.form._subs, window, cx);
        self.form.providers.push(row);
        self.recompute(cx);
    }

    fn remove_provider(&mut self, uid: usize, cx: &mut Context<Self>) {
        self.form.providers.retain(|r| r.uid != uid);
        self.recompute(cx);
    }

    fn toggle_key(&mut self, uid: usize, window: &mut Window, cx: &mut Context<Self>) {
        if let Some(r) = self.form.providers.iter_mut().find(|r| r.uid == uid) {
            r.show_key = !r.show_key;
            let show = r.show_key;
            r.key.update(cx, |s, cx| s.set_masked(!show, window, cx));
            cx.notify();
        }
    }

    fn add_model(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let id = self.form.model_input.read(cx).value().trim().to_string();
        let provider_ids: Vec<String> = self.draft.providers.iter().map(|p| p.id.clone()).collect();
        match id.split_once('/') {
            Some((p, m)) if !p.is_empty() && !m.is_empty() => {
                if !provider_ids.iter().any(|x| x == p) {
                    state::toast(
                        ToastKind::Warning,
                        format!("There is no provider “{p}” yet; the model is added anyway."),
                        cx,
                    );
                }
            }
            _ => {
                state::toast(ToastKind::Warning, "Use the form provider/model", cx);
                return;
            }
        }
        if self.form.models.contains(&id) {
            state::toast(ToastKind::Info, "That model is already in the list", cx);
            return;
        }
        self.form.models.push(id.clone());
        if self.form.default_model.is_empty() {
            self.form.default_model = id;
        }
        self.form
            .model_input
            .update(cx, |s, cx| s.set_value("", window, cx));
        self.form.refresh_selects(window, cx);
        self.recompute(cx);
    }

    fn remove_model(&mut self, model: &str, window: &mut Window, cx: &mut Context<Self>) {
        self.form.models.retain(|m| m != model);
        if self.form.default_model == model {
            self.form.default_model = self.form.models.first().cloned().unwrap_or_default();
        }
        if self.form.fast_model == model {
            self.form.fast_model.clear();
        }
        self.form.refresh_selects(window, cx);
        self.recompute(cx);
    }

    /// Sends a tiny request to the model (with the saved config) and shows latency or error.
    fn test_model(&mut self, model: String, cx: &mut Context<Self>) {
        if model.is_empty() || matches!(self.tests.get(&model), Some(TestState::Running)) {
            return;
        }
        self.tests.insert(model.clone(), TestState::Running);
        cx.notify();
        let engine = state::engine(cx);
        let m = model.clone();
        let job = cx.background_spawn(async move {
            let t = Instant::now();
            let r = engine.test_model(&m).map_err(|e| format!("{e:#}"));
            (r, t.elapsed())
        });
        cx.spawn(async move |this, cx| {
            let (r, took) = job.await;
            let _ = this.update(cx, |this, cx| {
                let st = match r {
                    Ok(reply) => TestState::Ok {
                        ms: took.as_millis(),
                        reply,
                    },
                    Err(e) => TestState::Err(e),
                };
                this.tests.insert(model, st);
                cx.notify();
            });
        })
        .detach();
    }

    fn import_v1(&mut self, cx: &mut Context<Self>) {
        let job = state::run_engine(cx, |e| {
            let mut c = e.config();
            let before = c.providers.len();
            let changed = c.import_harness_v1()?;
            let added = c.providers.len() - before;
            if changed {
                e.set_config(c)?;
            }
            Ok((changed, added))
        });
        cx.spawn(async move |_, cx| {
            if let Some((changed, added)) = job.await {
                let _ = cx.update(|cx| {
                    if changed {
                        state::toast(
                            ToastKind::Success,
                            format!("Imported {added} provider(s) from Harness v1"),
                            cx,
                        );
                    } else {
                        state::toast(ToastKind::Info, "Nothing new to import from Harness v1", cx);
                    }
                });
            }
        })
        .detach();
    }

    // ---- execution / prices ---------------------------------------------------------

    fn load_skills(&mut self, cx: &mut Context<Self>) {
        let engine = state::engine(cx);
        let job = cx.background_spawn(async move { engine.skills() });
        cx.spawn(async move |this, cx| {
            let skills = job.await;
            let _ = this.update(cx, |this, cx| {
                this.skills = skills;
                cx.notify();
            });
        })
        .detach();
    }

    /// Lets the user pick a skills folder with the native directory dialog, then saves it.
    /// If the dialog is unavailable, falls back to an empty, focused full-width input row.
    fn add_skill_dir(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let picked = cx.prompt_for_paths(PathPromptOptions {
            files: false,
            directories: true,
            multiple: false,
            prompt: Some("Add skills folder".into()),
        });
        cx.spawn_in(window, async move |this, cx| {
            let paths = match picked.await {
                Ok(Ok(Some(paths))) => paths,
                Ok(Ok(None)) => return, // cancelled
                _ => vec![],            // dialog failed or is unavailable: type the path instead
            };
            let _ = this.update_in(cx, |this, window, cx| {
                let path = paths
                    .first()
                    .map(|p| p.display().to_string())
                    .unwrap_or_default();
                let uid = this.form.uid();
                let e = make_input(
                    &mut this.form._subs,
                    "~/skills",
                    path.clone(),
                    false,
                    window,
                    cx,
                );
                this.form.skill_dirs.push((uid, e.clone()));
                this.recompute(cx);
                if path.is_empty() {
                    e.update(cx, |s, cx| s.focus(window, cx));
                } else {
                    this.save_skill_dirs(cx);
                }
            });
        })
        .detach();
    }

    fn remove_skill_dir(&mut self, uid: usize, cx: &mut Context<Self>) {
        self.form.skill_dirs.retain(|(u, _)| *u != uid);
        self.recompute(cx);
        self.save_skill_dirs(cx);
    }

    /// Saves only the skill folders (other unsaved edits stay in the draft), then rescans.
    /// Empty rows are dropped by `collect`; repeats are ignored.
    fn save_skill_dirs(&mut self, cx: &mut Context<Self>) {
        let mut dirs: Vec<String> = vec![];
        for d in &self.draft.skill_dirs {
            if !dirs.contains(d) {
                dirs.push(d.clone());
            }
        }
        if dirs == self.saved.skill_dirs {
            return;
        }
        let mut cfg = self.saved.clone();
        cfg.skill_dirs = dirs;
        let job = state::run_engine(cx, move |e| e.set_config(cfg));
        cx.spawn(async move |this, cx| {
            let ok = job.await.is_some();
            let _ = this.update(cx, |this, cx| {
                if ok {
                    this.load_skills(cx);
                    state::toast(ToastKind::Success, "Skill folders saved", cx);
                }
                cx.notify();
            });
        })
        .detach();
    }

    fn add_price(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let uid = self.form.uid();
        let row = price_row(
            uid,
            "",
            &Price {
                input: 0.0,
                output: 0.0,
            },
            &mut self.form._subs,
            window,
            cx,
        );
        self.form.prices.push(row);
        self.recompute(cx);
    }

    fn remove_price(&mut self, uid: usize, cx: &mut Context<Self>) {
        self.form.prices.retain(|r| r.uid != uid);
        self.recompute(cx);
    }

    // ---- app & updates ------------------------------------------------------------

    /// Loads branch / head / behind off the UI thread.
    fn refresh_update(&mut self, cx: &mut Context<Self>) {
        if self.upd.loading {
            return;
        }
        self.upd.loading = true;
        self.upd.requested = true;
        cx.notify();
        let engine = state::engine(cx);
        let job = cx
            .background_spawn(async move { update::status(&engine).map_err(|e| format!("{e:#}")) });
        cx.spawn(async move |this, cx| {
            let st = job.await;
            let _ = this.update(cx, |this, cx| {
                this.upd.status = Some(st);
                this.upd.loading = false;
                cx.notify();
            });
        })
        .detach();
    }

    /// Pulls and builds on a background thread, streaming progress lines; on success starts
    /// the new binary and quits.
    fn run_update(&mut self, cx: &mut Context<Self>) {
        if self.upd.running {
            return;
        }
        self.upd.running = true;
        self.upd.log = vec!["Starting update…".into()];
        cx.notify();
        let engine = state::engine(cx);
        let (tx, rx) = smol::channel::unbounded::<UpdateMsg>();
        std::thread::spawn(move || {
            let lines = tx.clone();
            let r = update::update(&engine, &mut |l| {
                let _ = lines.send_blocking(UpdateMsg::Line(l.to_string()));
            });
            let _ = tx.send_blocking(UpdateMsg::Done(r.map_err(|e| format!("{e:#}"))));
        });
        cx.spawn(async move |this, cx| {
            while let Ok(msg) = rx.recv().await {
                match msg {
                    UpdateMsg::Line(l) => {
                        let _ = this.update(cx, |this, cx| {
                            this.upd.log.push(l);
                            cx.notify();
                        });
                    }
                    UpdateMsg::Done(Ok(path)) => {
                        match std::process::Command::new(&path).spawn() {
                            Ok(_) => {
                                let _ = cx.update(|cx| cx.quit());
                            }
                            Err(e) => {
                                let _ = this.update(cx, |this, cx| {
                                    this.upd.running = false;
                                    this.upd.log.push(format!(
                                        "Built {}, but starting it failed: {e}",
                                        path.display()
                                    ));
                                    state::toast(
                                        ToastKind::Danger,
                                        format!("Couldn't start the new build: {e}"),
                                        cx,
                                    );
                                    cx.notify();
                                });
                            }
                        }
                        break;
                    }
                    UpdateMsg::Done(Err(e)) => {
                        let _ = this.update(cx, |this, cx| {
                            this.upd.running = false;
                            this.upd.log.push(format!("Failed: {e}"));
                            state::toast(
                                ToastKind::Danger,
                                "Update failed. See the log in Settings.",
                                cx,
                            );
                            cx.notify();
                        });
                        break;
                    }
                }
            }
        })
        .detach();
    }

    fn select_section(&mut self, s: Section, cx: &mut Context<Self>) {
        self.section = s;
        if s == Section::App && !self.upd.requested {
            self.refresh_update(cx);
        }
        cx.notify();
    }
}

// ---------------------------------------------------------------------------
// Validation helpers
// ---------------------------------------------------------------------------

fn url_error(url: &str) -> Option<String> {
    let rest = url
        .strip_prefix("https://")
        .or_else(|| url.strip_prefix("http://"));
    match rest {
        Some(r) if !r.is_empty() && !r.starts_with('/') && !url.contains(char::is_whitespace) => {
            None
        }
        _ if url.is_empty() => Some("Required".into()),
        _ => Some("Must be a http:// or https:// URL".into()),
    }
}

/// Parses a non-negative number; empty counts as 0. Records an error under `key` otherwise.
fn parse_money(s: &str, key: &str, err: &mut Errors) -> Option<f64> {
    if s.is_empty() {
        return Some(0.0);
    }
    match s.parse::<f64>() {
        Ok(v) if v.is_finite() && v >= 0.0 => Some(v),
        _ => {
            err.insert(key.to_string(), "A number, 0 or more".into());
            None
        }
    }
}

// ---------------------------------------------------------------------------
// Rendering helpers
// ---------------------------------------------------------------------------

fn hint(text: impl Into<SharedString>) -> Div {
    div().text_label().fg_muted().child(text.into())
}

/// Label, optional muted hint, the control, and a validation message.
fn block(
    label: &'static str,
    hint_text: Option<&'static str>,
    control: impl IntoElement,
    error: Option<&String>,
) -> Div {
    div()
        .flex()
        .flex_col()
        .gap(theme::S2)
        .child(
            div()
                .flex()
                .items_baseline()
                .gap(theme::S2)
                .child(div().text_body_strong().fg_secondary().child(label))
                .when_some(hint_text, |d, h| d.child(hint(h))),
        )
        .child(control)
        .when_some(error.cloned(), |d, e| {
            d.child(div().text_label().text_color(theme::DANGER).child(e))
        })
}

fn mono_row(id: &'static str, text: String, cx: &mut Context<SettingsScreen>) -> Div {
    let copy = text.clone();
    div()
        .flex()
        .items_center()
        .gap(theme::S2)
        .child(
            div()
                .flex_1()
                .min_w_0()
                .h(theme::CONTROL_H_SM)
                .flex()
                .items_center()
                .px(theme::S3)
                .rounded(theme::RADIUS_MD)
                .bg(theme::SURFACE_RAISED)
                .border_1()
                .border_color(theme::LINE)
                .text_mono()
                .truncate()
                .child(text),
        )
        .child(
            Button::new(id, "Copy")
                .small()
                .icon(AppIcon::Copy)
                .on_click(cx.listener(move |_, _, _, cx| {
                    cx.write_to_clipboard(ClipboardItem::new_string(copy.clone()));
                    state::toast(ToastKind::Info, "Copied", cx);
                })),
        )
}

impl Render for SettingsScreen {
    fn render(&mut self, _window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let header = PageHeader::new("Settings")
            .subtitle(Config::path().display().to_string())
            .when(self.dirty(), |h| {
                h.meta(Badge::new("Unsaved changes").accent().dot())
            });

        let nav = div()
            .w(px(216.))
            .flex_none()
            .h_full()
            .flex()
            .flex_col()
            .gap(px(2.))
            .p(theme::S3)
            .border_r_1()
            .border_color(theme::LINE)
            .children(Section::ALL.into_iter().map(|s| {
                let active = s == self.section;
                div()
                    .id(SharedString::from(format!("settings-nav-{s:?}")))
                    .flex()
                    .items_center()
                    .gap(theme::S2)
                    .h(theme::NAV_ITEM_H)
                    .px(theme::S3)
                    .rounded(theme::RADIUS_MD)
                    .text_body_strong()
                    .cursor_pointer()
                    .when(active, |d| {
                        d.bg(theme::SURFACE_HOVER).text_color(theme::TEXT)
                    })
                    .when(!active, |d| {
                        d.text_color(theme::TEXT_MUTED)
                            .hover(|s| s.bg(theme::SURFACE_RAISED).text_color(theme::TEXT))
                    })
                    .child(icon_sized(s.icon(), theme::ICON_SM))
                    .child(s.label())
                    .on_click(cx.listener(move |this, _, _, cx| this.select_section(s, cx)))
            }));

        let content = match self.section {
            Section::Models => self.render_models(cx).into_any_element(),
            Section::Rules => self.render_rules(cx).into_any_element(),
            Section::Budgets => self.render_budgets(cx).into_any_element(),
            Section::Execution => self.render_execution(cx).into_any_element(),
            Section::Prices => self.render_prices(cx).into_any_element(),
            Section::App => self.render_app(cx).into_any_element(),
        };

        let show_bar = self.dirty() || !self.errors.is_empty();
        let main = div()
            .relative()
            .flex_1()
            .min_w_0()
            .h_full()
            .child(
                div()
                    .id("settings-scroll")
                    .size_full()
                    .overflow_y_scroll()
                    .px(theme::S6)
                    .pt(theme::S6)
                    .pb(px(96.))
                    .child(
                        div()
                            .max_w(px(760.))
                            .flex()
                            .flex_col()
                            .gap(theme::S4)
                            .child(content),
                    ),
            )
            .when(show_bar, |d| d.child(self.render_bar(cx)));

        screen()
            .child(header)
            .child(div().flex_1().min_h_0().flex().child(nav).child(main))
    }
}

impl SettingsScreen {
    /// Sticky "Unsaved changes" bar that slides up while the draft differs from the config.
    fn render_bar(&self, cx: &mut Context<Self>) -> impl IntoElement {
        let problems = self.errors.len();
        div()
            .absolute()
            .bottom(theme::S5)
            .left_0()
            .right_0()
            .flex()
            .justify_center()
            .child(
                div()
                    .id("settings-bar")
                    .flex()
                    .items_center()
                    .gap(theme::S4)
                    .h(px(52.))
                    .pl(theme::S4)
                    .pr(theme::S2)
                    .rounded(theme::RADIUS_XL)
                    .bg(theme::SURFACE_HOVER)
                    .border_1()
                    .border_color(theme::LINE_STRONG)
                    .shadow(vec![gpui::BoxShadow {
                        color: theme::SCRIM,
                        offset: gpui::point(px(0.), px(8.)),
                        blur_radius: px(24.),
                        spread_radius: px(0.),
                    }])
                    .child(if problems > 0 {
                        Badge::new(format!(
                            "{problems} field{} to fix",
                            if problems == 1 { "" } else { "s" }
                        ))
                        .danger()
                        .dot()
                    } else {
                        Badge::new("Unsaved changes").accent().dot()
                    })
                    .child(
                        Button::new("settings-revert", "Revert")
                            .ghost()
                            .on_click(cx.listener(|this, _, window, cx| this.revert(window, cx))),
                    )
                    .child(
                        Button::new("settings-save", "Save")
                            .primary()
                            .icon(AppIcon::Check)
                            .disabled(problems > 0 || self.saving)
                            .on_click(cx.listener(|this, _, _, cx| this.save(cx))),
                    )
                    .with_animation(
                        "settings-bar-in",
                        Animation::new(theme::DUR_OVERLAY).with_easing(theme::ease_out),
                        |d, t| d.opacity(t).mb(px(-24.) * (1.0 - t)),
                    ),
            )
    }

    fn err(&self, key: &str) -> Option<&String> {
        self.errors.get(key)
    }

    // ---- Models & providers ------------------------------------------------------

    fn render_models(&self, cx: &mut Context<Self>) -> impl IntoElement {
        let f = &self.form;
        let dirty = self.dirty();

        let provider_rows = f
            .providers
            .iter()
            .enumerate()
            .map(|(ix, r)| {
                let uid = r.uid;
                let env = r.env.read(cx).value().trim().to_string();
                let has_key = !r.key.read(cx).unmask_value().trim().is_empty();
                let key_note = if has_key {
                    "Key stored in the config file".to_string()
                } else if !env.is_empty() {
                    if std::env::var(&env).map(|v| !v.is_empty()).unwrap_or(false) {
                        format!("Reads {env} (set)")
                    } else {
                        format!("{env} is not set in this environment")
                    }
                } else {
                    "No key: requests are sent without one".to_string()
                };
                div()
                    .flex()
                    .flex_col()
                    .gap(theme::S3)
                    .p(theme::S3)
                    .rounded(theme::RADIUS_LG)
                    .border_1()
                    .border_color(theme::LINE)
                    .bg(theme::SURFACE_RAISED)
                    .child(
                        div()
                            .flex()
                            .gap(theme::S3)
                            .child(div().w(px(160.)).flex_none().child(block(
                                "Id",
                                None,
                                input::field(&r.id),
                                self.err(&format!("prov.{ix}.id")),
                            )))
                            .child(div().flex_1().min_w_0().child(block(
                                "Base URL",
                                None,
                                input::field(&r.url),
                                self.err(&format!("prov.{ix}.url")),
                            ))),
                    )
                    .child(
                        div()
                            .flex()
                            .gap(theme::S3)
                            .child(
                                div().flex_1().min_w_0().child(block(
                                    "API key",
                                    None,
                                    div()
                                        .flex()
                                        .items_center()
                                        .gap(theme::S2)
                                        .child(div().flex_1().min_w_0().child(input::field(&r.key)))
                                        .child(
                                            Button::new(
                                                ("key-toggle", uid),
                                                if r.show_key { "Hide" } else { "Show" },
                                            )
                                            .ghost()
                                            .on_click(
                                                cx.listener(move |this, _, window, cx| {
                                                    this.toggle_key(uid, window, cx)
                                                }),
                                            ),
                                        ),
                                    None,
                                )),
                            )
                            .child(div().w(px(220.)).flex_none().child(block(
                                "or env variable",
                                None,
                                input::field(&r.env),
                                None,
                            ))),
                    )
                    .child(
                        div()
                            .flex()
                            .items_center()
                            .gap(theme::S2)
                            .child(Badge::new("OpenAI-compatible"))
                            .child(hint(key_note))
                            .child(div().flex_1())
                            .child(
                                Button::new(("prov-del", uid), "Remove")
                                    .small()
                                    .danger()
                                    .icon(AppIcon::Trash)
                                    .on_click(cx.listener(move |this, _, _, cx| {
                                        this.remove_provider(uid, cx)
                                    })),
                            ),
                    )
            })
            .collect::<Vec<_>>();

        let providers = card()
            .gap(theme::S3)
            .child(
                SectionHeader::new("Providers")
                    .icon(AppIcon::Bot)
                    .meta("OpenAI-compatible endpoints")
                    .when(self.v1_available, |h| {
                        h.child(
                            Button::new("import-v1", "Import from Harness v1")
                                .small()
                                .icon(AppIcon::Download)
                                .tooltip("Adds the providers (and a default model) from ~/.config/harness/config.toml and saves")
                                .on_click(cx.listener(|this, _, _, cx| this.import_v1(cx))),
                        )
                    })
                    .child(Button::new("prov-add", "Add provider").small().icon(AppIcon::Plus).on_click(cx.listener(|this, _, window, cx| this.add_provider(window, cx)))),
            )
            .when(f.providers.is_empty(), |d| {
                d.child(EmptyState::new(AppIcon::Bot, "No provider yet", "Add an OpenAI-compatible provider (base URL and key) to start chats and runs.").compact().id("empty-providers"))
            })
            .children(provider_rows);

        let model_rows =
            f.models
                .iter()
                .enumerate()
                .map(|(ix, m)| {
                    let model = m.clone();
                    let del = m.clone();
                    let test = m.clone();
                    let is_default = *m == f.default_model;
                    let st = self.tests.get(m);
                    div()
                        .flex()
                        .items_center()
                        .gap(theme::S2)
                        .min_h(px(36.))
                        .px(theme::S3)
                        .rounded(theme::RADIUS_MD)
                        .border_1()
                        .border_color(theme::LINE)
                        .child(div().flex_1().min_w_0().text_mono().truncate().child(model))
                        .when(is_default, |d| d.child(Badge::new("default").accent()))
                        .child(test_result(st, ix))
                        .child(
                            Button::new(("model-test", ix), "Test")
                                .small()
                                .icon(AppIcon::Zap)
                                .disabled(matches!(st, Some(TestState::Running)))
                                .tooltip(if dirty {
                                    "Tests use the saved settings; save first to test edits"
                                } else {
                                    "Sends a tiny request to the model"
                                })
                                .on_click(cx.listener(move |this, _, _, cx| {
                                    this.test_model(test.clone(), cx)
                                })),
                        )
                        .child(
                            IconButton::new(("model-del", ix), AppIcon::X)
                                .small()
                                .tooltip("Remove from the list")
                                .on_click(cx.listener(move |this, _, window, cx| {
                                    this.remove_model(&del, window, cx)
                                })),
                        )
                })
                .collect::<Vec<_>>();

        let models = card()
            .gap(theme::S4)
            .child(
                SectionHeader::new("Models")
                    .icon(AppIcon::Sliders)
                    .meta("shown in pickers"),
            )
            .child(
                div()
                    .flex()
                    .gap(theme::S4)
                    .child(
                        div().flex_1().min_w_0().child(block(
                            "Default model",
                            None,
                            div()
                                .flex()
                                .gap(theme::S2)
                                .child(div().flex_1().min_w_0().child(sel::select(&f.default_sel)))
                                .child(self.test_button(
                                    "default-test",
                                    f.default_model.clone(),
                                    cx,
                                )),
                            None,
                        )),
                    )
                    .child(
                        div().flex_1().min_w_0().child(block(
                            "Fast model",
                            Some("titles, summaries"),
                            div()
                                .flex()
                                .gap(theme::S2)
                                .child(div().flex_1().min_w_0().child(sel::select(&f.fast_sel)))
                                .child(self.test_button("fast-test", f.fast_model.clone(), cx)),
                            None,
                        )),
                    ),
            )
            .child(
                div()
                    .flex()
                    .flex_col()
                    .gap(theme::S2)
                    .child(
                        div()
                            .flex()
                            .items_baseline()
                            .gap(theme::S2)
                            .child(div().text_body_strong().fg_secondary().child("Model list"))
                            .child(hint(if f.models.is_empty() {
                                "Empty: the app asks each provider for its models"
                            } else {
                                "provider/model"
                            })),
                    )
                    .children(model_rows)
                    .child(
                        div()
                            .flex()
                            .gap(theme::S2)
                            .child(div().flex_1().min_w_0().child(input::field(&f.model_input)))
                            .child(
                                Button::new("model-add", "Add")
                                    .icon(AppIcon::Plus)
                                    .on_click(cx.listener(|this, _, window, cx| {
                                        this.add_model(window, cx)
                                    })),
                            ),
                    ),
            );

        div()
            .flex()
            .flex_col()
            .gap(theme::S4)
            .child(providers)
            .child(models)
    }

    /// A "Test" button for a model that may be empty (disabled then).
    fn test_button(
        &self,
        id: &'static str,
        model: String,
        cx: &mut Context<Self>,
    ) -> impl IntoElement {
        let running = matches!(self.tests.get(&model), Some(TestState::Running));
        let ready = !model.is_empty();
        let result = self.tests.get(&model);
        div()
            .flex()
            .items_center()
            .gap(theme::S2)
            .child(test_result(result, 1000))
            .child(
                Button::new(id, "Test")
                    .icon(AppIcon::Zap)
                    .disabled(!ready || running)
                    .on_click(
                        cx.listener(move |this, _, _, cx| this.test_model(model.clone(), cx)),
                    ),
            )
    }

    // ---- Workspace rules ----------------------------------------------------------

    fn render_rules(&self, cx: &mut Context<Self>) -> impl IntoElement {
        let text = self.form.rules.read(cx).value();
        let chars = text.chars().count();
        let lines = text.lines().filter(|l| !l.trim().is_empty()).count();
        card()
            .gap(theme::S3)
            .child(
                SectionHeader::new("Workspace rules")
                    .icon(AppIcon::File)
                    .child(Badge::new("prompt layer 1").accent()),
            )
            .child(hint("Sent with every call of every agent and chat: language, voice, house rules. Agents can't edit this layer. A rule that only one agent needs belongs in its role (layer 2, on the Agents screen)."))
            .child(input::composer_field(&self.form.rules))
            .child(hint(format!("{lines} rule line{} · ~{} tokens · {chars} chars", if lines == 1 { "" } else { "s" }, chars.div_ceil(4))))
    }

    // ---- Budgets --------------------------------------------------------------------

    fn render_budgets(&self, _cx: &mut Context<Self>) -> impl IntoElement {
        let f = &self.form;
        card()
            .gap(theme::S4)
            .child(SectionHeader::new("Budgets").icon(AppIcon::Activity).meta("USD, 0 = none"))
            .child(
                div()
                    .flex()
                    .gap(theme::S4)
                    .child(div().flex_1().child(block("Daily budget", None, input::field(&f.daily), self.err("budget.daily"))))
                    .child(div().flex_1().child(block("Monthly budget", None, input::field(&f.monthly), self.err("budget.monthly")))),
            )
            .child(hint("Display only: the sidebar shows spend against these numbers. Nothing is stopped when they are reached; limits that pause or stop work are policies, which are not part of this release."))
    }

    // ---- Execution --------------------------------------------------------------------

    fn render_execution(&self, cx: &mut Context<Self>) -> impl IntoElement {
        let f = &self.form;
        let dir_rows = f.skill_dirs.iter().map(|(uid, e)| {
            let uid = *uid;
            div()
                .flex()
                .items_center()
                .gap(theme::S2)
                .child(
                    div()
                        .flex_1()
                        .min_w_0()
                        .child(block_plain(input::field_sm(e), None)),
                )
                .child(
                    IconButton::new(("dir-del", uid), AppIcon::X)
                        .small()
                        .tooltip("Remove folder")
                        .on_click(
                            cx.listener(move |this, _, _, cx| this.remove_skill_dir(uid, cx)),
                        ),
                )
        });
        let run = card()
            .gap(theme::S4)
            .child(SectionHeader::new("Execution").icon(AppIcon::Terminal))
            .child(block(
                "Parallel agent slots",
                Some("tasks running at once; the rest queue"),
                div().w(px(120.)).child(input::field(&f.slots)),
                self.err("slots"),
            ))
            .child(block(
                "Render command",
                Some("HTML to PNG for Render steps"),
                input::field(&f.render_cmd),
                self.err("render"),
            ))
            .child(hint("Placeholders: {in} input HTML file, {out} output PNG, {w} width, {h} height. Empty uses chromium from PATH, else nix-shell -p chromium."));

        let skills = card()
            .gap(theme::S3)
            .child(
                SectionHeader::new("Skills")
                    .icon(AppIcon::Sparkles)
                    .meta(format!("{} found", self.skills.len()))
                    .child(
                        Button::new("dir-add", "Add folder")
                            .small()
                            .icon(AppIcon::Plus)
                            .on_click(
                                cx.listener(|this, _, window, cx| this.add_skill_dir(window, cx)),
                            ),
                    ),
            )
            .child(hint(
                "Folders with <name>/SKILL.md. Earlier folders win on name clashes.",
            ))
            .children(dir_rows)
            .child(divider())
            .when(self.skills.is_empty(), |d| {
                d.child(hint("No skills found in the saved folders."))
            })
            .children(self.skills.iter().map(|s| {
                div()
                    .flex()
                    .items_baseline()
                    .gap(theme::S3)
                    .child(
                        div()
                            .w(px(180.))
                            .flex_none()
                            .text_mono()
                            .truncate()
                            .child(s.name.clone()),
                    )
                    .child(
                        div()
                            .flex_1()
                            .min_w_0()
                            .text_label()
                            .fg_muted()
                            .truncate()
                            .child(s.description.clone()),
                    )
            }));
        div()
            .flex()
            .flex_col()
            .gap(theme::S4)
            .child(run)
            .child(skills)
    }

    // ---- Prices -------------------------------------------------------------------------

    fn render_prices(&self, cx: &mut Context<Self>) -> impl IntoElement {
        let rows = self.form.prices.iter().enumerate().map(|(ix, r)| {
            let uid = r.uid;
            let e = |k: &str| self.err(&format!("price.{ix}.{k}"));
            div()
                .flex()
                .items_start()
                .gap(theme::S3)
                .child(
                    div()
                        .flex_1()
                        .min_w_0()
                        .child(block_plain(input::field_sm(&r.family), e("family"))),
                )
                .child(
                    div()
                        .w(px(140.))
                        .flex_none()
                        .child(block_plain(input::field_sm(&r.input), e("input"))),
                )
                .child(
                    div()
                        .w(px(140.))
                        .flex_none()
                        .child(block_plain(input::field_sm(&r.output), e("output"))),
                )
                .child(
                    IconButton::new(("price-del", uid), AppIcon::X)
                        .small()
                        .tooltip("Remove")
                        .on_click(cx.listener(move |this, _, _, cx| this.remove_price(uid, cx))),
                )
        });
        card()
            .gap(theme::S3)
            .child(
                SectionHeader::new("Prices")
                    .icon(AppIcon::Sliders)
                    .meta("USD per 1M tokens")
                    .child(Button::new("price-add", "Add family").small().icon(AppIcon::Plus).on_click(cx.listener(|this, _, window, cx| this.add_price(window, cx)))),
            )
            .child(hint("Costs are estimated from these. A row matches every model whose id contains its family name (the longest match wins); models without a match cost nothing."))
            .child(
                div()
                    .flex()
                    .gap(theme::S3)
                    .child(div().flex_1().text_label().fg_muted().child("Model family"))
                    .child(div().w(px(140.)).flex_none().text_label().fg_muted().child("Input"))
                    .child(div().w(px(140.)).flex_none().text_label().fg_muted().child("Output"))
                    .child(div().w(px(28.)).flex_none()),
            )
            .children(rows)
    }

    // ---- App & updates ----------------------------------------------------------------------

    fn render_app(&self, cx: &mut Context<Self>) -> impl IntoElement {
        let f = &self.form;
        let u = &self.upd;
        let repo = card()
            .gap(theme::S3)
            .child(SectionHeader::new("This app's repository").icon(AppIcon::GitBranch))
            .child(hint("Used by self-improvement pipelines and by updates. Empty uses the folder this build came from."))
            .child(input::field(&f.self_repo));

        let status: AnyElement = match (&u.status, u.loading) {
            (None, true) => hint("Checking…").into_any_element(),
            (None, false) => hint("Not checked yet.").into_any_element(),
            (Some(Err(e)), _) => div()
                .text_label()
                .text_color(theme::DANGER)
                .child(e.clone())
                .into_any_element(),
            (Some(Ok(s)), _) => div()
                .flex()
                .flex_col()
                .gap(theme::S2)
                .child(
                    div()
                        .flex()
                        .items_center()
                        .gap(theme::S2)
                        .child(
                            Badge::new(s.branch.clone())
                                .mono()
                                .leading(icon_sized(AppIcon::GitBranch, px(12.))),
                        )
                        .child(if s.behind > 0 {
                            Badge::new(format!(
                                "{} commit{} behind",
                                s.behind,
                                if s.behind == 1 { "" } else { "s" }
                            ))
                            .warning()
                            .dot()
                        } else {
                            Badge::new("Up to date").success().dot()
                        })
                        .when(s.dirty, |d| d.child(Badge::new("local changes").danger())),
                )
                .child(
                    div()
                        .text_mono()
                        .fg_secondary()
                        .truncate()
                        .child(s.head.clone()),
                )
                .child(
                    div()
                        .text_label()
                        .fg_muted()
                        .truncate()
                        .child(s.repo.display().to_string()),
                )
                .into_any_element(),
        };
        let behind_dirty = matches!(&u.status, Some(Ok(s)) if s.behind > 0 && s.dirty);
        let can_update = !u.running && matches!(&u.status, Some(Ok(_))) && !behind_dirty;
        let updates = card()
            .gap(theme::S3)
            .child(
                SectionHeader::new("Updates")
                    .icon(AppIcon::Download)
                    .child(
                        Button::new("upd-refresh", "Refresh")
                            .small()
                            .icon(AppIcon::RotateCcw)
                            .disabled(u.loading || u.running)
                            .on_click(cx.listener(|this, _, _, cx| this.refresh_update(cx))),
                    )
                    .child(
                        Button::new("upd-run", "Update & restart")
                            .small()
                            .primary()
                            .disabled(!can_update)
                            .tooltip(if behind_dirty {
                                "The repo has uncommitted changes; commit or stash them first"
                            } else {
                                "Pulls, builds a release and restarts into it"
                            })
                            .on_click(cx.listener(|this, _, _, cx| this.run_update(cx))),
                    ),
            )
            .child(status)
            .when(!u.log.is_empty(), |d| {
                d.child(CodeBlock::new("update-log", u.log.join("\n")).max_height(px(220.)))
            });

        let paths = card()
            .gap(theme::S3)
            .child(SectionHeader::new("Data on disk").icon(AppIcon::Folder))
            .child(block(
                "Config",
                Some("config.toml holds API keys"),
                mono_row(
                    "copy-config",
                    harness_core::paths::config_dir().display().to_string(),
                    cx,
                ),
                None,
            ))
            .child(block(
                "Data",
                Some("database, worktrees, renders"),
                mono_row(
                    "copy-data",
                    harness_core::paths::data_dir().display().to_string(),
                    cx,
                ),
                None,
            ));

        div()
            .flex()
            .flex_col()
            .gap(theme::S4)
            .child(repo)
            .child(updates)
            .child(paths)
    }
}

/// `840 ms`, `2.4 s`.
fn fmt_ms(ms: u128) -> String {
    if ms < 1000 {
        format!("{ms} ms")
    } else {
        format!("{:.1} s", ms as f64 / 1000.0)
    }
}

/// A control with an optional validation message and no label (table rows).
fn block_plain(control: impl IntoElement, error: Option<&String>) -> Div {
    div()
        .flex()
        .flex_col()
        .gap(theme::S1)
        .child(control)
        .when_some(error.cloned(), |d, e| {
            d.child(div().text_label().text_color(theme::DANGER).child(e))
        })
}

/// The outcome of a model test: spinner text, latency badge, or the error.
fn test_result(st: Option<&TestState>, ix: usize) -> AnyElement {
    match st {
        None => div().into_any_element(),
        Some(TestState::Running) => div()
            .flex()
            .items_center()
            .gap(theme::S1)
            .text_label()
            .fg_muted()
            .child(icon_sized(AppIcon::Loader, px(12.)))
            .child("Testing…")
            .into_any_element(),
        Some(TestState::Ok { ms, reply }) => {
            let tip = format!("Replied: {}", reply.trim());
            div()
                .id(("test-ok", ix))
                .child(Badge::new(format!("ok · {}", fmt_ms(*ms))).success().dot())
                .tooltip(move |window, cx| Tooltip::new(tip.clone()).build(window, cx))
                .into_any_element()
        }
        Some(TestState::Err(e)) => {
            let full = e.clone();
            let short: String = e.lines().next().unwrap_or("").chars().take(60).collect();
            div()
                .id(("test-err", ix))
                .max_w(px(260.))
                .text_label()
                .text_color(theme::DANGER)
                .truncate()
                .child(short)
                .tooltip(move |window, cx| Tooltip::new(full.clone()).build(window, cx))
                .into_any_element()
        }
    }
}
