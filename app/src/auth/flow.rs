//! The steps of connecting an account or signing out, as a state machine
//! both front ends drive: `pathfinder auth login` draws it under the prompt,
//! `/login` in the TUI's bottom panel. Logging in takes two steps, how to
//! connect and then (for an API key) which provider; keys go in masked
//! [`Field`]s; the work itself is a [`Job`] the front end runs
//! ([`super::connect::run`]).

use crossterm::event::{KeyCode, KeyEvent};

use super::accounts::{
    METHOD_API_KEY, Row, RowView, State, logout_items, method_items, provider_items,
};
use super::check_key_shape;
use super::connect::{server_key_name, server_name};
use crate::config::Route;
use crate::picker::{Field, Outcome, Picker};

/// A key, kept out of `Debug` output and logs.
#[derive(Clone, PartialEq, Eq)]
pub struct Secret(String);

impl Secret {
    pub fn new(s: &str) -> Self {
        Self(s.trim().to_string())
    }

    pub fn expose(&self) -> &str {
        &self.0
    }
}

impl std::fmt::Debug for Secret {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("Secret(hidden)")
    }
}

/// Whose key a key step saves.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum KeyTarget {
    Route(Route),
    /// The gate's decision service (TypeSafe Jev).
    Gate,
    /// A configured endpoint, by id.
    Endpoint(String),
}

impl KeyTarget {
    pub fn of(row: &Row) -> Option<Self> {
        match row {
            Row::Gate => Some(Self::Gate),
            Row::Endpoint(id) => Some(Self::Endpoint(id.clone())),
            other => other.route().map(Self::Route),
        }
    }

    pub fn row(&self) -> Row {
        match self {
            Self::Route(Route::Openai) => Row::OpenaiApi,
            Self::Route(_) => Row::AnthropicApi,
            Self::Gate => Row::Gate,
            Self::Endpoint(id) => Row::Endpoint(id.clone()),
        }
    }
}

/// Work a step hands to the front end.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Job {
    /// Browser sign-in; the URL comes back through `on_url`.
    SignInChatgpt,
    /// Checks the key, then saves it.
    SaveKey { target: KeyTarget, key: Secret },
    /// Checks the key in `var`; with `save`, also keeps it in the auth file.
    UseEnv {
        target: KeyTarget,
        var: &'static str,
        save: bool,
    },
    /// Checks a server, then adds it to `config.json` (its key, if any, to
    /// the auth file).
    AddServer {
        url: String,
        name: String,
        key: Option<Secret>,
    },
    /// Removes only this row's credential.
    SignOut(Row),
    /// Removes a server from `config.json`, with its key.
    RemoveServer(String),
}

/// What a key asks the front end to do.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Action {
    None,
    /// Esc at the first step: close, nothing changed.
    Close,
    /// Esc while a job runs: stop it. When [`Flow::is_closed`], close too.
    Cancel,
    Run(Job),
    /// Put this (the sign-in address) on the clipboard.
    Copy(String),
}

/// Said when a key is pasted into a list's filter (and dropped).
pub const PASTED_KEY: &str =
    "That looks like a key, so it wasn't typed here. Pick the provider first, then paste it.";

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Mode {
    Login,
    Logout,
    /// The gate's key, from Settings: set, replace or remove it.
    Gate,
}

/// One step; Esc goes back to the one below it.
#[derive(Debug, Clone)]
pub enum Step {
    /// Login's first step: the ChatGPT plan, an API key, or your own server.
    Methods(Picker),
    /// After "Use an API key": the providers (and servers), with a filter.
    Providers(Picker),
    /// Logout's list: what Pathfinder holds.
    Accounts(Picker),
    /// The gate's key is stored: replace it or remove it.
    GateMenu {
        hint: String,
        picker: Picker,
    },
    /// The key's environment variable is set: use it, or paste another.
    UseEnv {
        target: KeyTarget,
        var: &'static str,
        picker: Picker,
    },
    Key {
        target: KeyTarget,
        field: Field,
    },
    Url(Field),
    Name {
        url: String,
        field: Field,
    },
    ServerKey {
        url: String,
        name: String,
        field: Field,
    },
    /// Signing out: what goes, before it goes.
    Confirm {
        row: Row,
        text: String,
        picker: Picker,
    },
    /// Text to read; Enter or Esc goes back.
    Info {
        title: String,
        text: String,
    },
    /// A job is running.
    Busy {
        title: String,
        text: String,
        /// What to say above the sign-in address.
        url_note: String,
        url: Option<String>,
    },
}

/// What to draw for the current step.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Screen<'a> {
    pub title: String,
    pub body: Vec<String>,
    pub list: Option<&'a Picker>,
    /// What is typed, as shown (dots when masked), and the cursor in chars.
    pub field: Option<(String, usize)>,
    /// An address to open, with what to say above it. Drawn unbroken where
    /// it can be (the terminal prints it on a line of its own).
    pub url: Option<(&'a str, &'a str)>,
    pub error: Option<&'a str>,
    pub hint: String,
}

/// Why a job failed, and whether it was the server's address (back to the
/// URL step) rather than what was typed last.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Failure {
    pub text: String,
    pub unreachable: bool,
}

impl Failure {
    pub fn of(e: &anyhow::Error) -> Self {
        Self {
            text: format!("{e:#}"),
            unreachable: e.downcast_ref::<super::connect::Unreachable>().is_some(),
        }
    }
}

impl From<String> for Failure {
    fn from(text: String) -> Self {
        Self {
            text,
            unreachable: false,
        }
    }
}

/// `text` with its first letter capitalised.
fn sentence(text: &str) -> String {
    let mut c = text.chars();
    match c.next() {
        Some(first) => first.to_uppercase().chain(c).collect(),
        None => String::new(),
    }
}

/// A server's base URL as typed, checked: one http(s) address with a host,
/// and a port when there is a `:`. The address, without a trailing `/`.
pub fn check_base_url(value: &str) -> Result<String, String> {
    let value = value.trim();
    if value.chars().any(char::is_whitespace) {
        return Err("The address has a space in it. Paste only the base URL.".into());
    }
    if !value.starts_with("http://") && !value.starts_with("https://") {
        return Err("The base URL starts with http:// or https://.".into());
    }
    if value.matches("://").count() > 1 {
        return Err("That looks like two addresses pasted together. Paste one.".into());
    }
    let authority = value
        .split_once("://")
        .map_or("", |(_, rest)| rest)
        .split(['/', '?', '#'])
        .next()
        .unwrap_or("");
    if authority.is_empty() || authority.starts_with(':') {
        return Err("The address has no host.".into());
    }
    if !authority.ends_with(']')
        && let Some((_, port)) = authority.rsplit_once(':')
        && (port.is_empty() || !port.chars().all(|c| c.is_ascii_digit()))
    {
        return Err("The port after `:` is missing or not a number.".into());
    }
    let parsed = reqwest::Url::parse(value).map_err(|e| format!("That isn't a URL: {e}."))?;
    if parsed.host_str().is_none_or(str::is_empty) {
        return Err("The address has no host.".into());
    }
    Ok(value.trim_end_matches('/').to_string())
}

/// Where to get a key, for the key step.
fn key_source(target: &KeyTarget) -> Option<&'static str> {
    match target {
        KeyTarget::Route(Route::Anthropic) => Some("https://console.anthropic.com/settings/keys"),
        KeyTarget::Route(Route::Openai) => Some("https://platform.openai.com/api-keys"),
        _ => None,
    }
}

/// A key step's title.
fn key_title(target: &KeyTarget) -> String {
    match target {
        KeyTarget::Route(_) => format!("{} API key", target.row().name()),
        KeyTarget::Gate => GATE_TITLE.into(),
        KeyTarget::Endpoint(id) => format!("Key for `{id}`"),
    }
}

/// The gate key's steps' title.
const GATE_TITLE: &str = "Gate key (TypeSafe Jev)";
const SERVER_TITLE: &str = "Add your own server";

#[derive(Debug, Clone)]
pub struct Flow {
    pub mode: Mode,
    stack: Vec<Step>,
    views: Vec<RowView>,
    pub error: Option<String>,
}

impl Flow {
    fn new(mode: Mode, views: Vec<RowView>) -> Self {
        Self {
            mode,
            stack: Vec::new(),
            views,
            error: None,
        }
    }

    /// Login's first step: how to connect.
    pub fn login(views: Vec<RowView>) -> Self {
        let mut flow = Self::new(Mode::Login, views);
        let picker = Picker::new(method_items(&flow.views));
        flow.stack.push(Step::Methods(picker));
        flow
    }

    /// What Pathfinder holds a credential (or a server) for, to remove one.
    pub fn logout(views: Vec<RowView>) -> Self {
        let mut flow = Self::new(Mode::Logout, views);
        let picker = Picker::new(logout_items(&flow.views));
        flow.stack.push(Step::Accounts(picker));
        flow
    }

    /// The gate's key, from Settings: replace or remove a stored one, else
    /// straight to typing it (or using `$TYPESAFE_API_KEY`).
    pub fn gate(views: Vec<RowView>) -> Self {
        let mut flow = Self::new(Mode::Gate, views);
        let stored = flow.view_of(&Row::Gate).and_then(|v| v.stored.clone());
        match stored {
            Some(hint) => flow.stack.push(Step::GateMenu {
                hint,
                picker: Picker::new(vec![
                    ("replace".into(), "Replace the key".into()),
                    ("remove".into(), "Remove the key".into()),
                ]),
            }),
            None => {
                flow.open(&Row::Gate);
            }
        }
        flow
    }

    /// Straight to `row`'s first step (`auth login anthropic`); Esc there
    /// closes. The action is a job to run when the row needs no input.
    pub fn at(mode: Mode, views: Vec<RowView>, row: &Row) -> (Self, Action) {
        let mut flow = Self::new(mode, views);
        let action = flow.open(row);
        (flow, action)
    }

    /// Nothing to pick from (signing out with nothing stored).
    pub fn is_empty(&self) -> bool {
        matches!(self.stack.as_slice(), [Step::Accounts(p)] if p.items.is_empty())
    }

    /// What to say when signing out has nothing to offer: also where the
    /// gate's key goes, when that is all Pathfinder holds.
    pub fn empty_text(&self) -> String {
        let gate_stored = self
            .views
            .iter()
            .any(|v| v.row == Row::Gate && v.stored.is_some());
        if gate_stored {
            "Pathfinder holds no sign-in or model key to remove. The gate key is removed in Settings (Ctrl+P → Gate key) or with `pathfinder auth remove typesafe`.".into()
        } else {
            "Pathfinder holds no sign-in or key to remove.".into()
        }
    }

    /// No step left: the front end closes (after a cancelled job that was
    /// the first step).
    pub fn is_closed(&self) -> bool {
        self.stack.is_empty()
    }

    pub fn step(&self) -> Option<&Step> {
        self.stack.last()
    }

    /// The list the current step picks from, if it is one.
    pub fn picker(&self) -> Option<&Picker> {
        match self.stack.last()? {
            Step::Methods(p)
            | Step::Providers(p)
            | Step::Accounts(p)
            | Step::GateMenu { picker: p, .. }
            | Step::UseEnv { picker: p, .. }
            | Step::Confirm { picker: p, .. } => Some(p),
            _ => None,
        }
    }

    fn picker_mut(&mut self) -> Option<&mut Picker> {
        match self.stack.last_mut()? {
            Step::Methods(p)
            | Step::Providers(p)
            | Step::Accounts(p)
            | Step::GateMenu { picker: p, .. }
            | Step::UseEnv { picker: p, .. }
            | Step::Confirm { picker: p, .. } => Some(p),
            _ => None,
        }
    }

    fn field_mut(&mut self) -> Option<&mut Field> {
        match self.stack.last_mut()? {
            Step::Key { field, .. }
            | Step::Url(field)
            | Step::Name { field, .. }
            | Step::ServerKey { field, .. } => Some(field),
            _ => None,
        }
    }

    /// Whether a job is running.
    pub fn busy(&self) -> bool {
        matches!(self.stack.last(), Some(Step::Busy { .. }))
    }

    fn view_of(&self, row: &Row) -> Option<&RowView> {
        self.views.iter().find(|v| &v.row == row)
    }

    fn busy_with(&mut self, title: String, text: String, job: Job) -> Action {
        self.stack.push(Step::Busy {
            title,
            text,
            url_note: String::new(),
            url: None,
        });
        Action::Run(job)
    }

    /// The browser sign-in, worded for whether a browser opens here.
    fn sign_in_chatgpt(&mut self) -> Action {
        let opens = crate::browser_opens();
        let (text, url_note) = if opens {
            (
                "Opening your browser to sign in with ChatGPT… Waiting for the browser.",
                "If the browser doesn't open, open this address:".to_string(),
            )
        } else {
            (
                "Open the sign-in address in a browser to sign in with ChatGPT. Waiting for the browser.",
                format!(
                    "The sign-in sends the browser back to 127.0.0.1 on this machine, so use a browser on this machine{}:",
                    if crate::over_ssh() {
                        " (over SSH, forward the port first: ssh -L <port>:127.0.0.1:<port>, the port being the one after 127.0.0.1: in the address)"
                    } else {
                        ""
                    }
                ),
            )
        };
        self.stack.push(Step::Busy {
            title: "Sign in with your ChatGPT plan".into(),
            text: text.into(),
            url_note,
            url: None,
        });
        Action::Run(Job::SignInChatgpt)
    }

    /// Signing out of `row`: what goes, then a confirmation.
    fn confirm_sign_out(&mut self, view: Option<RowView>, row: &Row) -> Action {
        let env_var = view.as_ref().and_then(|v| v.env);
        let stored = view.as_ref().and_then(|v| v.stored.clone());
        let env_note = env_var
            .map(|v| format!(" ${v} is set too and stays in use until you unset it."))
            .unwrap_or_default();
        let (text, items): (String, Vec<(String, String)>) = match (row, &stored) {
            (Row::Endpoint(id), Some(hint)) => (
                format!("`{id}` uses the key {hint} from Pathfinder's auth file."),
                vec![
                    (
                        "key".into(),
                        "Remove only its key (the server stays in config.json)".into(),
                    ),
                    ("server".into(), "Remove the server and its key".into()),
                ],
            ),
            (Row::Endpoint(id), None) => (
                format!("`{id}` is in config.json."),
                vec![("server".into(), format!("Remove `{id}` from config.json"))],
            ),
            (Row::Chatgpt, Some(_)) => (
                "Signs out of ChatGPT here and revokes the token with OpenAI.".into(),
                vec![("yes".into(), "Sign out of ChatGPT".into())],
            ),
            (_, Some(hint)) => (
                format!(
                    "Removes the {} key {hint} from Pathfinder's auth file.{env_note}",
                    row.name()
                ),
                vec![("yes".into(), format!("Remove the key {hint}"))],
            ),
            (_, None) => {
                let text = match env_var {
                    Some(var) => format!(
                        "This key comes from ${var} in your environment, not from Pathfinder's auth file. Unset it in your shell to stop using it."
                    ),
                    None => "Pathfinder holds nothing for this account.".into(),
                };
                self.stack.push(Step::Info {
                    title: row.name(),
                    text,
                });
                return Action::None;
            }
        };
        self.stack.push(Step::Confirm {
            row: row.clone(),
            text,
            picker: Picker::new(items),
        });
        Action::None
    }

    /// The step for a picked row.
    fn open(&mut self, row: &Row) -> Action {
        let view = self.view_of(row).cloned();
        let state = view.as_ref().map(|v| v.state.clone());
        if let Some(text) = row.sign_in_hint(state == Some(State::Installed)) {
            self.stack.push(Step::Info {
                title: row.name(),
                text,
            });
            return Action::None;
        }
        if self.mode == Mode::Logout {
            return self.confirm_sign_out(view, row);
        }
        match row {
            Row::Chatgpt => self.sign_in_chatgpt(),
            Row::Advanced => {
                self.stack.push(Step::Url(Field::plain("")));
                Action::None
            }
            Row::Endpoint(id) if state == Some(State::NoKeyNeeded) => {
                self.stack.push(Step::Info {
                    title: row.name(),
                    text: format!(
                        "`{id}` takes no key, so there is nothing to sign in to. Its entry is in config.json; /logout or `pathfinder auth logout {id}` removes it."
                    ),
                });
                Action::None
            }
            _ => {
                let Some(target) = KeyTarget::of(row) else {
                    return Action::None;
                };
                self.stack.push(match view.as_ref().and_then(|v| v.env) {
                    Some(var) => Step::UseEnv {
                        target,
                        var,
                        picker: Picker::new(vec![
                            (
                                "use".into(),
                                format!("Use ${var} (checked now; it stays in your shell)"),
                            ),
                            (
                                "save".into(),
                                format!("Save ${var}'s key in Pathfinder's auth file"),
                            ),
                            (
                                "paste".into(),
                                format!(
                                    "Paste a different key (${var} still wins while it is set)"
                                ),
                            ),
                        ]),
                    },
                    None => Step::Key {
                        target,
                        field: Field::masked(),
                    },
                });
                Action::None
            }
        }
    }

    /// Esc: back a step, or close at the first.
    fn back(&mut self) -> Action {
        let was_busy = self.busy();
        self.stack.pop();
        if was_busy {
            return Action::Cancel;
        }
        if self.stack.is_empty() {
            Action::Close
        } else {
            Action::None
        }
    }

    pub fn on_key(&mut self, key: KeyEvent) -> Action {
        let edited = matches!(
            key.code,
            KeyCode::Char(_) | KeyCode::Backspace | KeyCode::Delete
        );
        let outcome = if let Some(p) = self.picker_mut() {
            let outcome = p.on_key(key);
            if edited && self.error.as_deref() == Some(PASTED_KEY) {
                self.error = None;
            }
            outcome
        } else if let Some(field) = self.field_mut() {
            let outcome = field.on_key(key);
            // Editing what was refused clears why it was.
            if edited && outcome == Outcome::Stay {
                self.error = None;
            }
            outcome
        } else {
            match self.stack.last() {
                None => return Action::Close,
                Some(Step::Info { .. }) => match key.code {
                    KeyCode::Enter | KeyCode::Esc => Outcome::Back,
                    _ => Outcome::Stay,
                },
                Some(Step::Busy { url, .. }) => match key.code {
                    KeyCode::Esc => Outcome::Back,
                    KeyCode::Char('c') if url.is_some() => {
                        return Action::Copy(url.clone().unwrap_or_default());
                    }
                    _ => Outcome::Stay,
                },
                Some(_) => Outcome::Stay,
            }
        };
        match outcome {
            Outcome::Stay => Action::None,
            Outcome::Back => {
                self.error = None;
                self.back()
            }
            Outcome::Submit(value) => self.submit(value),
        }
    }

    pub fn on_paste(&mut self, text: &str) {
        // A key pasted where a list filters is dropped, never shown: the
        // provider is picked first, then the key goes in a masked field.
        if self.picker().is_some() && super::accounts::looks_like_key(text) {
            if self.picker().is_some_and(Picker::filterable) {
                self.error = Some(PASTED_KEY.into());
            }
            return;
        }
        if let Some(p) = self.picker_mut() {
            p.on_paste(text);
        } else if let Some(field) = self.field_mut() {
            field.on_paste(text);
            self.error = None;
        }
    }

    fn submit(&mut self, value: String) -> Action {
        self.error = None;
        let Some(step) = self.stack.last().cloned() else {
            return Action::Close;
        };
        match step {
            Step::Methods(_) if value == METHOD_API_KEY => {
                let picker = Picker::new(provider_items(&self.views)).filtering();
                self.stack.push(Step::Providers(picker));
                Action::None
            }
            Step::Methods(_) | Step::Providers(_) | Step::Accounts(_) => {
                match Row::from_id(&value) {
                    Some(row) => self.open(&row),
                    None => Action::None,
                }
            }
            Step::GateMenu { .. } => {
                if value == "remove" {
                    let view = self.view_of(&Row::Gate).cloned();
                    self.confirm_sign_out(view, &Row::Gate)
                } else {
                    self.stack.push(Step::Key {
                        target: KeyTarget::Gate,
                        field: Field::masked(),
                    });
                    Action::None
                }
            }
            Step::UseEnv { target, var, .. } => match value.as_str() {
                "paste" => {
                    self.stack.push(Step::Key {
                        target,
                        field: Field::masked(),
                    });
                    Action::None
                }
                choice => {
                    let save = choice == "save";
                    self.busy_with(
                        key_title(&target),
                        format!("Checking the key in ${var}…"),
                        Job::UseEnv { target, var, save },
                    )
                }
            },
            Step::Key { target, .. } => {
                if let Err(e) = check_key_shape(&value) {
                    self.error = Some(format!(
                        "{}. Paste it again, or Esc to go back.",
                        sentence(&e)
                    ));
                    return Action::None;
                }
                self.busy_with(
                    key_title(&target),
                    format!("Checking the key with {}…", target.row().name()),
                    Job::SaveKey {
                        target,
                        key: Secret::new(&value),
                    },
                )
            }
            Step::Url(_) => {
                let url = match check_base_url(&value) {
                    Ok(url) => url,
                    Err(e) => {
                        self.error = Some(e);
                        return Action::None;
                    }
                };
                let suggested = server_name(&url);
                self.stack.push(Step::Name {
                    url,
                    field: Field::plain(&suggested),
                });
                Action::None
            }
            Step::Name { url, .. } => {
                if let Err(e) = server_key_name(&value) {
                    self.error = Some(format!("{}.", sentence(&e.to_string())));
                    return Action::None;
                }
                self.stack.push(Step::ServerKey {
                    url,
                    name: value,
                    field: Field::masked(),
                });
                Action::None
            }
            Step::ServerKey { url, name, .. } => {
                let key = (!value.is_empty()).then(|| Secret::new(&value));
                self.busy_with(
                    SERVER_TITLE.into(),
                    format!("Checking {url}…"),
                    Job::AddServer { url, name, key },
                )
            }
            Step::Confirm { row, .. } => {
                let job = match (&row, value.as_str()) {
                    (Row::Endpoint(id), "server") => Job::RemoveServer(id.clone()),
                    _ => Job::SignOut(row.clone()),
                };
                let text = match &job {
                    Job::RemoveServer(id) => format!("Removing `{id}`…"),
                    _ => format!("Signing out of {}…", row.name()),
                };
                self.busy_with(row.name(), text, job)
            }
            Step::Info { .. } | Step::Busy { .. } => self.back(),
        }
    }

    /// The running job gave a URL to visit (the browser sign-in).
    pub fn set_url(&mut self, link: &str) {
        if let Some(Step::Busy { url, .. }) = self.stack.last_mut() {
            *url = Some(link.to_string());
        }
    }

    /// The running job failed: back to the step that started it, with why;
    /// when the server couldn't be reached, back to its address. False when
    /// there is no step left to show it on.
    pub fn fail(&mut self, failure: impl Into<Failure>) -> bool {
        let failure = failure.into();
        if self.busy() {
            self.stack.pop();
        }
        if failure.unreachable && self.stack.iter().any(|s| matches!(s, Step::Url(_))) {
            while !matches!(self.stack.last(), Some(Step::Url(_))) {
                self.stack.pop();
            }
        }
        self.error = Some(failure.text);
        !self.stack.is_empty()
    }

    /// The hint line for a list: Esc cancels at the first step, else goes
    /// back.
    fn list_hint(&self, filter: bool) -> String {
        let typed = self.picker().is_some_and(|p| !p.query.is_empty());
        let esc = if typed {
            "Esc clears"
        } else if self.stack.len() <= 1 && self.mode != Mode::Gate {
            "Esc cancel"
        } else {
            "Esc back"
        };
        if filter {
            format!("↑↓ navigate · type to filter · Enter select · {esc}")
        } else {
            format!("↑↓ navigate · Enter select · {esc}")
        }
    }

    pub fn screen(&self) -> Screen<'_> {
        let error = self.error.as_deref();
        let mut screen = Screen {
            title: String::new(),
            body: Vec::new(),
            list: None,
            field: None,
            url: None,
            error,
            hint: String::new(),
        };
        let Some(step) = self.stack.last() else {
            return screen;
        };
        // From Settings, Esc goes back there.
        let esc = if self.stack.len() <= 1 && self.mode != Mode::Gate {
            "Esc cancel"
        } else {
            "Esc back"
        };
        if let Some(p) = self.picker() {
            screen.list = Some(p);
            screen.hint = self.list_hint(p.filterable());
        }
        let field = |f: &Field| Some((f.shown(), f.cursor()));
        match step {
            Step::Methods(_) => screen.title = "Select how to connect:".into(),
            Step::Providers(_) => screen.title = "Select a provider:".into(),
            Step::Accounts(p) => {
                screen.title = "Select what to sign out of:".into();
                if p.items.is_empty() {
                    screen.body.push(self.empty_text());
                }
            }
            Step::GateMenu { hint, .. } => {
                screen.title = format!("{GATE_TITLE}:");
                screen.body.push(format!(
                    "The key {hint} is in Pathfinder's auth file. The gate asks this service before showing a candidate's answer."
                ));
            }
            Step::UseEnv { target, var, .. } => {
                screen.title = format!("{}:", key_title(target));
                screen
                    .body
                    .push(format!("${var} is set in your environment."));
            }
            Step::Key { target, field: f } => {
                screen.title = format!("{}:", key_title(target));
                screen.body.push(match target {
                    KeyTarget::Route(r) => format!(
                        "Paste your {} API key. It is checked by listing your models, then kept in Pathfinder's auth file (readable by you only).",
                        r.provider_name()
                    ),
                    KeyTarget::Gate => "Paste your TypeSafe Jev key. It is checked with one small request, then kept in Pathfinder's auth file. The gate asks this service before showing a candidate's answer.".into(),
                    KeyTarget::Endpoint(id) => format!(
                        "Paste the API key for `{id}`. It is checked by listing the server's models, then kept in Pathfinder's auth file."
                    ),
                });
                if let Some(hint) = self
                    .view_of(&target.row())
                    .filter(|_| *target != KeyTarget::Gate)
                    .and_then(|v| v.stored.as_deref())
                {
                    screen
                        .body
                        .push(format!("This replaces the stored key {hint}."));
                }
                if let Some(at) = key_source(target) {
                    screen.body.push(format!("Get one at {at}"));
                }
                screen.field = field(f);
                screen.hint = format!("Enter check and save · {esc}");
            }
            Step::Url(f) => {
                screen.title = format!("{SERVER_TITLE}: base URL");
                screen.body.push(
                    "Any OpenAI-compatible server, up to and including /v1: http://localhost:11434/v1 for Ollama, https://openrouter.ai/api/v1 for OpenRouter.".into(),
                );
                screen.field = field(f);
                screen.hint = format!("Enter next · {esc}");
            }
            Step::Name { url, field: f } => {
                screen.title = format!("{SERVER_TITLE}: short name");
                screen.body.push(format!(
                    "A short name for {url}; its models show as <name>:<model>."
                ));
                screen.field = field(f);
                screen.hint = format!("Enter next · {esc}");
            }
            Step::ServerKey { name, field: f, .. } => {
                screen.title = format!("{SERVER_TITLE}: API key");
                screen.body.push(format!(
                    "The API key for `{name}`, if it needs one; leave it empty if not."
                ));
                screen.field = field(f);
                screen.hint = format!("Enter check and add · {esc}");
            }
            Step::Confirm { row, text, picker } => {
                screen.title = match row {
                    Row::Gate => "Remove the gate key:".into(),
                    _ => format!("Sign out of {}:", row.name()),
                };
                screen.body.push(text.clone());
                let nav = if picker.items.len() > 1 {
                    "↑↓ navigate · "
                } else {
                    ""
                };
                screen.hint = match row {
                    Row::Chatgpt => format!("{nav}Enter sign out · {esc}"),
                    _ => format!("{nav}Enter remove · {esc}"),
                };
            }
            Step::Info { title, text } => {
                screen.title = format!("{title}:");
                screen.body.push(text.clone());
                screen.hint = "Enter or Esc back".into();
            }
            Step::Busy {
                title,
                text,
                url_note,
                url,
            } => {
                screen.title = format!("{title}:");
                screen.body.push(text.clone());
                screen.url = url.as_deref().map(|u| (url_note.as_str(), u));
                screen.hint = if url.is_some() {
                    "c copy address · Esc cancel".into()
                } else {
                    "Esc cancel".into()
                };
            }
        }
        screen
    }
}

#[cfg(test)]
mod tests {
    use std::sync::Arc;

    use crossterm::event::{KeyCode, KeyModifiers};

    use super::super::accounts::rows;
    use super::super::secrets::{MemoryStore, SecretStore};
    use super::super::{Credentials, KeySource};
    use super::*;
    use crate::config::{CliAccounts, Config};

    fn key(code: KeyCode) -> KeyEvent {
        KeyEvent::new(code, KeyModifiers::NONE)
    }

    fn views(env: &dyn Fn(&str) -> Option<String>) -> Vec<RowView> {
        let store = Arc::new(MemoryStore::default());
        store
            .set("openai-api-key", "sk-proj-stored-key-0001")
            .unwrap();
        let creds = Credentials::for_tests(store, Arc::new(|_| Ok(None)));
        rows(&creds, &Config::default(), CliAccounts::default(), env)
    }

    fn pick(flow: &mut Flow, id: &str) -> Action {
        let Some(p) = flow.picker() else {
            panic!("not on a list: {:?}", flow.step());
        };
        let i = p
            .matches()
            .iter()
            .position(|&m| p.items[m].0 == id)
            .unwrap();
        flow.on_key(key(KeyCode::Home));
        for _ in 0..i {
            flow.on_key(key(KeyCode::Down));
        }
        flow.on_key(key(KeyCode::Enter))
    }

    fn type_text(flow: &mut Flow, text: &str) {
        for c in text.chars() {
            flow.on_key(key(KeyCode::Char(c)));
        }
    }

    /// Login's first step, then "Use an API key", then `id`.
    fn pick_provider(flow: &mut Flow, id: &str) -> Action {
        assert_eq!(pick(flow, METHOD_API_KEY), Action::None);
        pick(flow, id)
    }

    #[test]
    fn login_takes_two_steps_and_esc_walks_back_through_them() {
        let mut flow = Flow::login(views(&|_| None));
        let screen = flow.screen();
        assert_eq!(screen.title, "Select how to connect:");
        assert_eq!(screen.hint, "↑↓ navigate · Enter select · Esc cancel");
        assert!(!screen.list.unwrap().filterable());
        assert_eq!(pick(&mut flow, METHOD_API_KEY), Action::None);
        let screen = flow.screen();
        assert_eq!(screen.title, "Select a provider:");
        assert!(screen.list.unwrap().filterable(), "a filter line");
        assert_eq!(
            screen.hint,
            "↑↓ navigate · type to filter · Enter select · Esc back"
        );
        assert_eq!(pick(&mut flow, "anthropic"), Action::None);
        assert!(matches!(flow.step(), Some(Step::Key { .. })));
        type_text(&mut flow, "abc");
        assert_eq!(flow.on_key(key(KeyCode::Esc)), Action::None);
        assert!(matches!(flow.step(), Some(Step::Providers(_))));
        assert_eq!(flow.on_key(key(KeyCode::Esc)), Action::None);
        assert!(matches!(flow.step(), Some(Step::Methods(_))));
        assert_eq!(flow.on_key(key(KeyCode::Esc)), Action::Close);
    }

    #[test]
    fn the_provider_filter_is_a_line_of_input() {
        let mut flow = Flow::login(views(&|_| None));
        pick(&mut flow, METHOD_API_KEY);
        type_text(&mut flow, "openx");
        assert_eq!(flow.picker().unwrap().query, "openx");
        assert_eq!(flow.picker().unwrap().selected(), None);
        // Enter on no match does nothing.
        assert_eq!(flow.on_key(key(KeyCode::Enter)), Action::None);
        flow.on_key(key(KeyCode::Backspace));
        assert_eq!(flow.picker().unwrap().query, "open");
        assert_eq!(flow.picker().unwrap().selected(), Some("openai"));
        // j is text here, not a move.
        flow.on_key(key(KeyCode::Char('j')));
        assert_eq!(flow.picker().unwrap().query, "openj");
        // Esc clears the filter first, then goes back.
        assert_eq!(flow.on_key(key(KeyCode::Esc)), Action::None);
        assert!(flow.picker().unwrap().query.is_empty());
        assert!(matches!(flow.step(), Some(Step::Providers(_))));
        flow.on_paste("anth");
        assert_eq!(flow.picker().unwrap().selected(), Some("anthropic"));
        assert_eq!(flow.on_key(key(KeyCode::Enter)), Action::None);
        assert!(matches!(
            flow.step(),
            Some(Step::Key {
                target: KeyTarget::Route(Route::Anthropic),
                ..
            })
        ));
    }

    #[test]
    fn a_key_pasted_into_the_provider_filter_is_dropped_never_shown() {
        let mut flow = Flow::login(views(&|_| None));
        pick(&mut flow, METHOD_API_KEY);
        let key_text = "sk-ant-api03-PASTEDKEY1234567890abcdef";
        flow.on_paste(key_text);
        assert!(flow.picker().unwrap().query.is_empty());
        let screen = flow.screen();
        assert_eq!(screen.error, Some(PASTED_KEY));
        assert!(!format!("{screen:?}").contains("PASTEDKEY"));
        assert!(!format!("{flow:?}").contains("PASTEDKEY"));
        // Typing again clears why; a name still filters.
        type_text(&mut flow, "anth");
        assert_eq!(flow.error, None);
        assert_eq!(flow.picker().unwrap().selected(), Some("anthropic"));
        // On the first step, which doesn't filter, a paste does nothing.
        let mut first = Flow::login(views(&|_| None));
        first.on_paste(key_text);
        assert_eq!(first.error, None);
        assert!(!format!("{first:?}").contains("PASTEDKEY"));
    }

    #[test]
    fn a_stored_key_is_replaced_and_says_so() {
        let mut flow = Flow::login(views(&|_| None));
        pick_provider(&mut flow, "openai");
        let body = flow.screen().body.join("\n");
        assert!(
            body.contains("This replaces the stored key …0001."),
            "{body}"
        );
        let mut fresh = Flow::login(views(&|_| None));
        pick_provider(&mut fresh, "anthropic");
        assert!(!fresh.screen().body.join(" ").contains("replaces"));
    }

    #[test]
    fn a_key_is_shape_checked_masked_and_handed_on() {
        let mut flow = Flow::login(views(&|_| None));
        pick_provider(&mut flow, "anthropic");
        assert_eq!(flow.screen().title, "Anthropic API key:");
        type_text(&mut flow, "short");
        assert_eq!(flow.on_key(key(KeyCode::Enter)), Action::None);
        assert_eq!(
            flow.error.as_deref(),
            Some("That looks too short for an API key. Paste it again, or Esc to go back.")
        );
        assert!(
            flow.screen()
                .body
                .iter()
                .any(|l| l.contains("console.anthropic.com"))
        );
        flow.on_paste("-and-now-long-enough-1234");
        assert!(flow.error.is_none());
        let screen = flow.screen();
        let (shown, _) = screen.field.clone().unwrap();
        assert!(shown.chars().all(|c| c == '•'));
        assert!(!format!("{screen:?}").contains("long-enough"));
        let action = flow.on_key(key(KeyCode::Enter));
        let Action::Run(Job::SaveKey { target, key: k }) = &action else {
            panic!("{action:?}");
        };
        assert_eq!(target, &KeyTarget::Route(Route::Anthropic));
        assert_eq!(k.expose(), "short-and-now-long-enough-1234");
        assert!(!format!("{action:?}").contains("long-enough"));
        assert!(flow.busy());
        // A failed check comes back to the key, with why.
        assert!(flow.fail("rejected".to_string()));
        assert!(matches!(flow.step(), Some(Step::Key { .. })));
        assert_eq!(flow.error.as_deref(), Some("rejected"));
        // Editing the key clears why it was refused.
        flow.on_key(key(KeyCode::Backspace));
        assert!(flow.error.is_none());
    }

    #[test]
    fn an_environment_key_is_offered_first() {
        let env = |v: &str| (v == "ANTHROPIC_API_KEY").then(|| "sk-ant-env-key-00000000".into());
        let mut flow = Flow::login(views(&env));
        pick_provider(&mut flow, "anthropic");
        let screen = flow.screen();
        assert!(screen.body[0].contains("$ANTHROPIC_API_KEY is set"));
        assert_eq!(
            flow.on_key(key(KeyCode::Enter)),
            Action::Run(Job::UseEnv {
                target: KeyTarget::Route(Route::Anthropic),
                var: "ANTHROPIC_API_KEY",
                save: false
            })
        );
        // Esc while it runs cancels it and goes back to the choice.
        assert_eq!(flow.on_key(key(KeyCode::Esc)), Action::Cancel);
        assert!(!flow.is_closed());
        flow.on_key(key(KeyCode::End));
        flow.on_key(key(KeyCode::Enter));
        assert!(matches!(flow.step(), Some(Step::Key { .. })));
    }

    #[test]
    fn chatgpt_signs_in_with_the_browser_and_shows_the_url() {
        let mut flow = Flow::login(views(&|_| None));
        assert_eq!(pick(&mut flow, "chatgpt"), Action::Run(Job::SignInChatgpt));
        let screen = flow.screen();
        assert_eq!(screen.title, "Sign in with your ChatGPT plan:");
        assert!(screen.body[0].contains("Waiting for the browser"));
        assert!(screen.url.is_none());
        assert_eq!(flow.on_key(key(KeyCode::Char('c'))), Action::None);
        flow.set_url("https://auth.example/authorize?x=1");
        let (_, url) = flow.screen().url.unwrap();
        assert_eq!(url, "https://auth.example/authorize?x=1");
        assert_eq!(flow.screen().hint, "c copy address · Esc cancel");
        // c copies it whole; Esc cancels, back at the first step.
        assert_eq!(
            flow.on_key(key(KeyCode::Char('c'))),
            Action::Copy("https://auth.example/authorize?x=1".into())
        );
        assert_eq!(flow.on_key(key(KeyCode::Esc)), Action::Cancel);
        assert!(matches!(flow.step(), Some(Step::Methods(_))));
        // Straight to it (`/login chatgpt`), Esc leaves nothing open.
        let (mut direct, action) = Flow::at(Mode::Login, views(&|_| None), &Row::Chatgpt);
        assert_eq!(action, Action::Run(Job::SignInChatgpt));
        assert_eq!(direct.on_key(key(KeyCode::Esc)), Action::Cancel);
        assert!(direct.is_closed());
    }

    #[test]
    fn login_lists_no_cli_rows_but_named_they_explain_their_own_login() {
        let mut flow = Flow::login(views(&|_| None));
        let ids = |flow: &Flow| -> Vec<String> {
            flow.picker()
                .unwrap()
                .items
                .iter()
                .map(|i| i.0.clone())
                .collect()
        };
        assert_eq!(ids(&flow), ["chatgpt", METHOD_API_KEY, "endpoint"]);
        pick(&mut flow, METHOD_API_KEY);
        assert_eq!(ids(&flow), ["anthropic", "openai"]);
        // `pathfinder auth login claude` still says where to sign in.
        let (named, action) = Flow::at(Mode::Login, views(&|_| None), &Row::ClaudeCode);
        assert_eq!(action, Action::None);
        let Some(Step::Info { text, .. }) = named.step() else {
            panic!()
        };
        assert!(text.contains("claude"));
    }

    #[test]
    fn the_gate_key_is_replaced_or_removed_from_settings() {
        // Not stored: straight to the masked key.
        let mut flow = Flow::gate(views(&|_| None));
        assert_eq!(flow.mode, Mode::Gate);
        assert!(matches!(
            flow.step(),
            Some(Step::Key {
                target: KeyTarget::Gate,
                ..
            })
        ));
        assert_eq!(flow.screen().title, "Gate key (TypeSafe Jev):");
        // Esc goes back to Settings, so the hint says back, not cancel.
        assert_eq!(flow.screen().hint, "Enter check and save · Esc back");
        flow.on_paste("ts-0123456789abcdefghij");
        assert!(flow.screen().field.unwrap().0.chars().all(|c| c == '•'));
        assert!(matches!(
            flow.on_key(key(KeyCode::Enter)),
            Action::Run(Job::SaveKey {
                target: KeyTarget::Gate,
                ..
            })
        ));
        let mut flow = Flow::gate(views(&|_| None));
        assert_eq!(flow.on_key(key(KeyCode::Esc)), Action::Close);
        // Stored: replace or remove.
        let stored = vec![RowView {
            stored: Some("…d509".into()),
            ..RowView::new(Row::Gate, State::NotSet)
        }];
        let mut flow = Flow::gate(stored.clone());
        assert!(flow.screen().body[0].contains("…d509"));
        assert_eq!(pick(&mut flow, "replace"), Action::None);
        assert!(matches!(flow.step(), Some(Step::Key { .. })));
        flow.on_key(key(KeyCode::Esc));
        assert_eq!(pick(&mut flow, "remove"), Action::None);
        assert!(matches!(flow.step(), Some(Step::Confirm { .. })));
        assert_eq!(flow.screen().title, "Remove the gate key:");
        assert_eq!(
            flow.on_key(key(KeyCode::Enter)),
            Action::Run(Job::SignOut(Row::Gate))
        );
        // $TYPESAFE_API_KEY is set: offered first, as for a provider.
        let env = |v: &str| (v == "TYPESAFE_API_KEY").then(|| "ts-env-0123456789abcd".into());
        let from_env = rows(
            &Credentials::for_tests(Arc::new(MemoryStore::default()), Arc::new(|_| Ok(None))),
            &Config::default(),
            CliAccounts::default(),
            &env,
        );
        let flow = Flow::gate(from_env);
        assert!(matches!(
            flow.step(),
            Some(Step::UseEnv {
                var: "TYPESAFE_API_KEY",
                ..
            })
        ));
    }

    #[test]
    fn a_base_url_is_one_address_with_a_host_and_a_port_when_it_says_so() {
        assert_eq!(
            check_base_url(" http://localhost:11434/v1/ ").unwrap(),
            "http://localhost:11434/v1"
        );
        assert!(check_base_url("https://openrouter.ai/api/v1").is_ok());
        assert!(check_base_url("http://[::1]:8000/v1").is_ok());
        for bad in [
            "localhost:11434/v1",
            "ftp://x/v1",
            "http://localhost:/v1",
            "http://localhost:abc/v1",
            "http://127.0.0.1:9/v1http://localhost:/v1",
            "http://a b/v1",
            "http:///v1",
        ] {
            assert!(check_base_url(bad).is_err(), "{bad}");
        }
    }

    #[test]
    fn an_unreachable_server_goes_back_to_its_address() {
        let mut flow = Flow::login(views(&|_| None));
        pick(&mut flow, "endpoint");
        type_text(&mut flow, "http://127.0.0.1:9/v1");
        flow.on_key(key(KeyCode::Enter));
        flow.on_key(key(KeyCode::Enter));
        assert!(matches!(
            flow.on_key(key(KeyCode::Enter)),
            Action::Run(Job::AddServer { .. })
        ));
        let e = anyhow::Error::new(super::super::connect::Unreachable(
            "Could not reach http://127.0.0.1:9/v1: connection refused. Is the server running?"
                .into(),
        ));
        assert!(flow.fail(Failure::of(&e)));
        let Some(Step::Url(f)) = flow.step() else {
            panic!("{:?}", flow.step())
        };
        assert_eq!(f.text(), "http://127.0.0.1:9/v1");
        assert!(
            flow.error
                .as_deref()
                .unwrap()
                .contains("connection refused")
        );
    }

    #[test]
    fn a_server_takes_a_url_a_name_and_an_optional_key() {
        let mut flow = Flow::login(views(&|_| None));
        pick(&mut flow, "endpoint");
        type_text(&mut flow, "localhost:11434/v1");
        flow.on_key(key(KeyCode::Enter));
        assert!(flow.error.is_some(), "needs a scheme");
        flow.on_key(key(KeyCode::Home));
        type_text(&mut flow, "http://");
        flow.on_key(key(KeyCode::Enter));
        let Some(Step::Name { field, .. }) = flow.step() else {
            panic!("{:?}", flow.step())
        };
        assert_eq!(field.text(), "localhost");
        // Esc goes back to the URL, as typed.
        flow.on_key(key(KeyCode::Esc));
        let Some(Step::Url(f)) = flow.step() else {
            panic!()
        };
        assert_eq!(f.text(), "http://localhost:11434/v1");
        flow.on_key(key(KeyCode::Enter));
        flow.on_key(key(KeyCode::Enter));
        assert!(matches!(flow.step(), Some(Step::ServerKey { .. })));
        assert_eq!(
            flow.on_key(key(KeyCode::Enter)),
            Action::Run(Job::AddServer {
                url: "http://localhost:11434/v1".into(),
                name: "localhost".into(),
                key: None
            })
        );
    }

    #[test]
    fn logout_lists_only_what_pathfinder_holds_and_asks_first() {
        let mut flow = Flow::logout(views(&|_| None));
        assert!(!flow.is_empty());
        let screen = flow.screen();
        assert_eq!(screen.title, "Select what to sign out of:");
        assert_eq!(screen.hint, "↑↓ navigate · Enter select · Esc cancel");
        // Only what /login creates: the stored OpenAI key here, no gate.
        let ids: Vec<&str> = screen
            .list
            .unwrap()
            .items
            .iter()
            .map(|i| i.0.as_str())
            .collect();
        assert_eq!(ids, ["openai"]);
        assert_eq!(pick(&mut flow, "openai"), Action::None);
        let screen = flow.screen();
        assert_eq!(screen.title, "Sign out of OpenAI:");
        assert_eq!(screen.hint, "Enter remove · Esc back");
        assert!(screen.body[0].contains("…0001"), "{:?}", screen.body);
        // Esc goes back without removing anything.
        assert_eq!(flow.on_key(key(KeyCode::Esc)), Action::None);
        assert_eq!(pick(&mut flow, "openai"), Action::None);
        assert_eq!(
            flow.on_key(key(KeyCode::Enter)),
            Action::Run(Job::SignOut(Row::OpenaiApi))
        );
        let empty = Flow::logout(rows(
            &Credentials::for_tests(Arc::new(MemoryStore::default()), Arc::new(|_| Ok(None))),
            &Config::default(),
            CliAccounts::default(),
            &|_| None,
        ));
        assert!(empty.is_empty());
        assert_eq!(
            empty.empty_text(),
            "Pathfinder holds no sign-in or key to remove."
        );
        // Only the gate's key stored: not listed, but said where it goes.
        let gate_only = Arc::new(MemoryStore::default());
        gate_only
            .set("typesafe-api-key", "ts-stored-gate-key-0001")
            .unwrap();
        let gate_only = Flow::logout(rows(
            &Credentials::for_tests(gate_only, Arc::new(|_| Ok(None))),
            &Config::default(),
            CliAccounts::default(),
            &|_| None,
        ));
        assert!(gate_only.is_empty());
        let text = gate_only.empty_text();
        assert!(text.contains("Settings (Ctrl+P → Gate key)"), "{text}");
        assert!(text.contains("auth remove typesafe"), "{text}");
        assert!(gate_only.screen().body[0].contains("Gate key"));
        let (mut named, action) = Flow::at(Mode::Logout, views(&|_| None), &Row::Gate);
        assert_eq!(action, Action::None);
        assert!(
            matches!(named.step(), Some(Step::Info { .. })),
            "nothing stored"
        );
        assert_eq!(named.on_key(key(KeyCode::Esc)), Action::Close);
    }

    #[test]
    fn a_stored_key_can_be_removed_while_its_env_var_is_used() {
        let env = |v: &str| (v == "OPENAI_API_KEY").then(|| "sk-proj-env-key-99999999".into());
        let mut flow = Flow::logout(views(&env));
        assert_eq!(pick(&mut flow, "openai"), Action::None);
        let screen = flow.screen();
        assert!(screen.body[0].contains("…0001"), "{:?}", screen.body);
        assert!(screen.body[0].contains("$OPENAI_API_KEY is set too"));
        assert_eq!(
            flow.on_key(key(KeyCode::Enter)),
            Action::Run(Job::SignOut(Row::OpenaiApi))
        );
        // Only in the environment: said, not removed.
        let only_env = rows(
            &Credentials::for_tests(Arc::new(MemoryStore::default()), Arc::new(|_| Ok(None))),
            &Config::default(),
            CliAccounts::default(),
            &env,
        );
        let (named, action) = Flow::at(Mode::Logout, only_env, &Row::OpenaiApi);
        assert_eq!(action, Action::None);
        let Some(Step::Info { text, .. }) = named.step() else {
            panic!()
        };
        assert!(text.contains("Unset it in your shell"));
    }

    #[test]
    fn a_server_can_lose_its_key_or_go_altogether() {
        let with_key = RowView {
            stored: Some("…ABCD".into()),
            ..RowView::new(
                Row::Endpoint("local".into()),
                State::Key {
                    hint: "…ABCD".into(),
                    source: KeySource::File,
                },
            )
        };
        let mut flow = Flow::logout(vec![with_key]);
        assert_eq!(pick(&mut flow, "endpoint:local"), Action::None);
        flow.on_key(key(KeyCode::End));
        assert_eq!(
            flow.on_key(key(KeyCode::Enter)),
            Action::Run(Job::RemoveServer("local".into()))
        );
        // Without a key it is still listed, to remove the server.
        let keyless = RowView::new(Row::Endpoint("lab".into()), State::NoKeyNeeded);
        let mut flow = Flow::logout(vec![keyless]);
        assert_eq!(pick(&mut flow, "endpoint:lab"), Action::None);
        assert_eq!(
            flow.on_key(key(KeyCode::Enter)),
            Action::Run(Job::RemoveServer("lab".into()))
        );
    }
}
