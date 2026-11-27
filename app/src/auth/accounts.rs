//! The accounts `pathfinder auth login`, `/login` and `auth status` list:
//! who runs the models and who pays, each with its state in a few words.
//! Keys only ever leave this module masked.

use crate::config::{Account, CliAccounts, Config, Route};
use crate::decide::Provider;

use super::{Credentials, KeySource, key_name};

/// A row of the account list.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Row {
    Chatgpt,
    ClaudeCode,
    CodexCli,
    AnthropicApi,
    OpenaiApi,
    /// A configured endpoint, by id.
    Endpoint(String),
    /// Adding another OpenAI-compatible service or your own server.
    Advanced,
    /// The decision service the gate asks (TypeSafe Jev's key).
    Gate,
}

impl Row {
    /// The id in pickers and on the command line.
    pub fn id(&self) -> String {
        match self {
            Self::Chatgpt => "chatgpt".into(),
            Self::ClaudeCode => "claude".into(),
            Self::CodexCli => "codex".into(),
            Self::AnthropicApi => "anthropic".into(),
            Self::OpenaiApi => "openai".into(),
            Self::Endpoint(id) => format!("endpoint:{id}"),
            Self::Advanced => "endpoint".into(),
            Self::Gate => "typesafe".into(),
        }
    }

    pub fn from_id(id: &str) -> Option<Self> {
        Some(match id {
            "chatgpt" => Self::Chatgpt,
            "claude" => Self::ClaudeCode,
            "codex" => Self::CodexCli,
            "anthropic" => Self::AnthropicApi,
            "openai" => Self::OpenaiApi,
            "endpoint" => Self::Advanced,
            "typesafe" => Self::Gate,
            _ => Self::Endpoint(id.strip_prefix("endpoint:")?.to_string()),
        })
    }

    /// The model account, for the model accounts.
    pub fn account(&self) -> Option<Account> {
        Some(match self {
            Self::Chatgpt => Account::ChatgptPlan,
            Self::ClaudeCode => Account::ClaudeCode,
            Self::CodexCli => Account::CodexCli,
            Self::AnthropicApi => Account::AnthropicApi,
            Self::OpenaiApi => Account::OpenaiApi,
            Self::Endpoint(id) => Account::Endpoint(id.clone()),
            Self::Advanced | Self::Gate => return None,
        })
    }

    /// The short name, in the login lists and in messages: who it is, not
    /// who pays (the first login step says that once).
    pub fn name(&self) -> String {
        match self {
            Self::Chatgpt => "ChatGPT plan".into(),
            Self::ClaudeCode => "Claude Code".into(),
            Self::CodexCli => "Codex CLI".into(),
            Self::AnthropicApi => "Anthropic".into(),
            Self::OpenaiApi => "OpenAI".into(),
            Self::Endpoint(id) => id.clone(),
            Self::Advanced => "Your own server".into(),
            Self::Gate => Provider::Typesafe.label().into(),
        }
    }

    /// The API route whose key this row holds.
    pub fn route(&self) -> Option<Route> {
        match self {
            Self::AnthropicApi => Some(Route::Anthropic),
            Self::OpenaiApi => Some(Route::Openai),
            _ => None,
        }
    }

    /// The environment variable that holds this row's key, if any.
    pub fn key_env(&self) -> Option<&'static str> {
        match self {
            Self::Gate => Some(Provider::Typesafe.key_env()),
            other => other.route().and_then(Route::key_env),
        }
    }

    /// For the rows whose CLI keeps its own login: how to sign in there.
    pub fn sign_in_hint(&self, installed: bool) -> Option<String> {
        let (name, bin, login, logout) = match self {
            Self::ClaudeCode => (
                "Claude Code",
                "claude",
                "run `claude`, then /login",
                "/logout",
            ),
            Self::CodexCli => ("Codex CLI", "codex", "run `codex login`", "`codex logout`"),
            _ => return None,
        };
        Some(if installed {
            format!(
                "{name} uses its own login, and Pathfinder runs it as it is. To sign in, {login} in a terminal; to sign out, {logout} there. Pathfinder never reads its credentials."
            )
        } else {
            format!(
                "{name} isn't installed (no `{bin}` on PATH). Install it, then {login} in a terminal; its models show up here on the next start."
            )
        })
    }
}

/// Names `pathfinder auth login|logout <account>` takes.
pub const NAMES: &str = "chatgpt|anthropic|openai|endpoint|typesafe";

/// Every name an account goes by, for messages.
pub const ALL_NAMES: &str = "chatgpt, anthropic, openai, endpoint (or server), typesafe (or gate), claude, codex, or a server's name";

/// Whether `s`, given where an account name goes, is more likely a key
/// pasted in the wrong place: then it is never echoed back.
pub fn looks_like_key(s: &str) -> bool {
    let s = s.trim();
    ["sk-", "ts-", "AIza", "sess-", "ghp_"]
        .iter()
        .any(|p| s.starts_with(p))
        || super::check_key_shape(s).is_ok()
}

/// What to say when `name` isn't an account: never the name itself when it
/// looks like a key, and never more than a few words of it.
pub fn unknown_account(name: &str) -> String {
    if looks_like_key(name) {
        return format!(
            "That looks like a key ({}), not an account name; it wasn't saved. Name the account: {ALL_NAMES}.",
            key_hint(name)
        );
    }
    let shown: String = name.chars().take(32).collect();
    let more = if name.chars().count() > 32 { "…" } else { "" };
    format!("No account `{shown}{more}`. Use {ALL_NAMES}.")
}

/// The row a command-line name means: `chatgpt`, `anthropic`, `openai`,
/// `endpoint` (or `server`) for a new server, `typesafe` (or `gate`), the
/// CLIs `claude` and `codex`, or a configured endpoint's id.
pub fn parse_account(name: &str, cfg: &Config) -> Option<Row> {
    let lower = name.to_ascii_lowercase();
    match lower.as_str() {
        "server" | "advanced" => return Some(Row::Advanced),
        "gate" => return Some(Row::Gate),
        "claude-code" => return Some(Row::ClaudeCode),
        "codex-cli" => return Some(Row::CodexCli),
        _ => {}
    }
    if let Some(row) = Row::from_id(&lower) {
        return match row {
            Row::Endpoint(id) => cfg
                .endpoints
                .iter()
                .any(|e| e.id == id)
                .then_some(Row::Endpoint(id)),
            other => Some(other),
        };
    }
    cfg.endpoints
        .iter()
        .find(|e| e.id == name)
        .map(|e| Row::Endpoint(e.id.clone()))
}

/// A row's state in a few words.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum State {
    SignedIn {
        email: Option<String>,
    },
    NotSignedIn,
    /// A key, by its last characters and where it is kept.
    Key {
        hint: String,
        source: KeySource,
    },
    Installed,
    NotInstalled,
    /// A server that takes no key.
    NoKeyNeeded,
    /// A server whose key entry is missing from the auth file.
    MissingKey,
    NotSet,
    /// The gate asks a model on one of your endpoints.
    GateEndpoint {
        model: String,
        endpoint: String,
    },
    /// The gate uses another decision key you have (Gemini, OpenAI).
    GateOtherKey {
        label: &'static str,
        source: KeySource,
    },
    /// You joined the beta: the shared gate.
    SharedGate,
    /// On-premises mode without an endpoint model for decisions.
    GateOffLocal,
    /// Nothing to show (the "add a server" row).
    Action,
}

/// `…1234`: enough to tell keys apart, never enough to use one.
pub fn key_hint(key: &str) -> String {
    let chars: Vec<char> = key.trim().chars().collect();
    let tail: String = chars[chars.len().saturating_sub(4)..].iter().collect();
    format!("…{tail}")
}

fn source_words(source: KeySource) -> String {
    match source {
        KeySource::Env(var) => format!("${var}"),
        KeySource::File => "auth file".into(),
    }
}

impl State {
    pub fn label(&self) -> String {
        match self {
            Self::SignedIn { email: Some(e) } => format!("signed in as {e}"),
            Self::SignedIn { email: None } => "signed in".into(),
            Self::NotSignedIn => "not signed in".into(),
            // Present, not necessarily working: the model count says that.
            Self::Key { hint, source } => format!("key {hint} · {}", source_words(*source)),
            Self::Installed => "installed · uses its own login".into(),
            Self::NotInstalled => "not installed".into(),
            Self::NoKeyNeeded => "no key needed".into(),
            Self::MissingKey => "key missing".into(),
            Self::NotSet => "not configured".into(),
            Self::GateEndpoint { model, endpoint } => {
                format!("your model {model} on `{endpoint}`")
            }
            Self::GateOtherKey { label, source } => {
                format!("uses your {label} key · {}", source_words(*source))
            }
            Self::SharedGate => "✓ the shared gate (joined the beta)".into(),
            Self::GateOffLocal => "off: on-premises mode needs `decisions` set".into(),
            Self::Action => "Ollama, OpenRouter, vLLM…".into(),
        }
    }

    /// The state in a word or two, for the login lists: no key hints, no
    /// counts. ✓ when it is there to use.
    pub fn short(&self) -> String {
        match self {
            Self::SignedIn { email: Some(e) } => format!("✓ signed in as {e}"),
            Self::SignedIn { email: None } => "✓ signed in".into(),
            Self::NotSignedIn => "not signed in".into(),
            Self::Key {
                source: KeySource::File,
                ..
            } => "✓ stored".into(),
            Self::Key {
                source: KeySource::Env(var),
                ..
            } => format!("✓ from ${var}"),
            Self::NotSet => "not configured".into(),
            // Nothing to set up: plain, not a ✓ (that marks what is stored).
            Self::NoKeyNeeded => "no key needed".into(),
            Self::MissingKey => "key missing".into(),
            Self::Installed => "✓ installed".into(),
            other => other.label(),
        }
    }

    /// Whether this row works now.
    pub fn ready(&self) -> bool {
        matches!(
            self,
            Self::SignedIn { .. }
                | Self::Key { .. }
                | Self::Installed
                | Self::NoKeyNeeded
                | Self::GateEndpoint { .. }
                | Self::GateOtherKey { .. }
                | Self::SharedGate
        )
    }
}

/// A row with its state.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RowView {
    pub row: Row,
    /// What is used now (an environment variable wins over the auth file).
    pub state: State,
    /// What Pathfinder's auth file holds for this row, as a hint (`…ABCD`,
    /// or the sign-in), even when something else is used instead: it is
    /// what signing out removes.
    pub stored: Option<String>,
    /// How many models the pickers list for this account, when counted.
    pub models: Option<usize>,
    /// The key's environment variable, when it is set (also when something
    /// else decides, as for the gate).
    pub env: Option<&'static str>,
}

impl RowView {
    /// A row whose stored entry, if any, is the one its state shows.
    pub fn new(row: Row, state: State) -> Self {
        let stored = match &state {
            State::SignedIn { .. } => Some(state.label()),
            State::Key {
                hint,
                source: KeySource::File,
            } => Some(hint.clone()),
            _ => None,
        };
        let env = match &state {
            State::Key {
                source: KeySource::Env(var),
                ..
            } => Some(*var),
            _ => None,
        };
        Self {
            row,
            state,
            stored,
            models: None,
            env,
        }
    }

    /// Whether signing out has something here to remove: a stored
    /// credential, or a server's entry in config.json.
    pub fn removable(&self) -> bool {
        self.stored.is_some() || matches!(self.row, Row::Endpoint(_))
    }

    /// What signing out removes, in a word or two.
    pub fn stored_label(&self) -> String {
        match (&self.row, &self.stored) {
            (Row::Chatgpt, _) => self.state.short(),
            (_, Some(_)) => "✓ stored".into(),
            (Row::Endpoint(_), None) => "no key".into(),
            _ => String::new(),
        }
    }
}

/// Reads environment variables; injected so tests don't depend on the shell.
pub type Env<'a> = &'a dyn Fn(&str) -> Option<String>;

/// The process environment, ignoring empty values.
pub fn process_env(var: &str) -> Option<String> {
    std::env::var(var)
        .ok()
        .map(|v| v.trim().to_string())
        .filter(|v| !v.is_empty())
}

/// Where a key is: the environment variable first (it wins at run time
/// too), then the auth file entry `name`.
fn key_state(creds: &Credentials, env: Env, var: Option<&'static str>, name: &str) -> State {
    if let Some(var) = var
        && let Some(v) = env(var)
    {
        return State::Key {
            hint: key_hint(&v),
            source: KeySource::Env(var),
        };
    }
    match creds.stored(name) {
        Some(k) => State::Key {
            hint: key_hint(&k),
            source: KeySource::File,
        },
        None => State::NotSet,
    }
}

/// Every row with its state, in the order the pickers show them. CLI
/// accounts show only whether the program is installed: their own
/// credentials are never read.
pub fn rows(creds: &Credentials, cfg: &Config, clis: CliAccounts, env: Env) -> Vec<RowView> {
    let session = creds.chatgpt();
    let stored_hint = |name: &str| creds.stored(name).map(|k| key_hint(&k));
    let mut out = vec![RowView::new(
        Row::Chatgpt,
        if session.signed_in() {
            State::SignedIn {
                email: session.registration().and_then(|r| r.email),
            }
        } else {
            State::NotSignedIn
        },
    )];
    for (row, installed) in [(Row::ClaudeCode, clis.claude), (Row::CodexCli, clis.codex)] {
        out.push(RowView::new(
            row,
            if installed {
                State::Installed
            } else {
                State::NotInstalled
            },
        ));
    }
    for row in [Row::AnthropicApi, Row::OpenaiApi] {
        let route = row.route().expect("an API row");
        let state = key_state(creds, env, route.key_env(), &key_name(route));
        out.push(RowView {
            stored: stored_hint(&key_name(route)),
            ..RowView::new(row, state)
        });
    }
    for ep in &cfg.endpoints {
        let state = match &ep.key {
            None => State::NoKeyNeeded,
            Some(name) => match creds.stored(name) {
                Some(k) => State::Key {
                    hint: key_hint(&k),
                    source: KeySource::File,
                },
                None => State::MissingKey,
            },
        };
        out.push(RowView::new(Row::Endpoint(ep.id.clone()), state));
    }
    out.push(RowView::new(Row::Advanced, State::Action));
    // The gate's own key, stored, even when a setting or another key decides.
    let gate_env = Some(Provider::Typesafe.key_env()).filter(|v| env(v).is_some());
    out.push(RowView {
        stored: stored_hint(Provider::Typesafe.key_name()),
        env: gate_env,
        ..RowView::new(Row::Gate, gate_state(creds, cfg, env))
    });
    out
}

/// Counts each account's models in `catalog` (what the pickers list).
pub fn with_models(mut views: Vec<RowView>, catalog: &[crate::config::ModelEntry]) -> Vec<RowView> {
    for v in &mut views {
        if let Some(account) = v.row.account() {
            v.models = Some(catalog.iter().filter(|m| Account::of(m) == account).count());
        }
    }
    views
}

/// What the gate asks, in the order the decision service is chosen.
fn gate_state(creds: &Credentials, cfg: &Config, env: Env) -> State {
    if let Some(d) = &cfg.decisions {
        return State::GateEndpoint {
            model: d.model.clone(),
            endpoint: d.endpoint.clone(),
        };
    }
    if cfg.local_only {
        return State::GateOffLocal;
    }
    let own = key_state(
        creds,
        env,
        Some(Provider::Typesafe.key_env()),
        Provider::Typesafe.key_name(),
    );
    if own.ready() {
        return own;
    }
    for p in [Provider::Openai, Provider::Gemini] {
        if let State::Key { source, .. } = key_state(creds, env, Some(p.key_env()), p.key_name()) {
            return State::GateOtherKey {
                label: p.label(),
                source,
            };
        }
    }
    if crate::telemetry::Consent::load().enrolled {
        return State::SharedGate;
    }
    State::NotSet
}

/// Whether any model account works now (the gate isn't one).
pub fn any_model_account(rows: &[RowView]) -> bool {
    rows.iter()
        .any(|r| r.row.account().is_some() && r.state.ready())
}

/// A row's state with what the last model refresh found: ✓ only when the
/// account lists models, since a key that is present may still be refused.
/// The detailed form, for `auth status`.
pub fn state_text(view: &RowView) -> String {
    let label = view.state.label();
    match view.models {
        Some(n) if n > 0 && view.state.ready() => {
            format!("✓ {label} · {n} model{}", if n == 1 { "" } else { "s" })
        }
        Some(0) if matches!(view.state, State::Key { .. } | State::SignedIn { .. }) => {
            format!("{label} · no models listed")
        }
        _ => label,
    }
}

/// Login's first step: how to connect. Who pays is said here, once.
pub fn method_items(views: &[RowView]) -> Vec<(String, String)> {
    let chatgpt = views
        .iter()
        .find(|v| v.row == Row::Chatgpt)
        .map_or_else(|| State::NotSignedIn.short(), |v| v.state.short());
    vec![
        (
            Row::Chatgpt.id(),
            with_note("Sign in with your ChatGPT plan", &chatgpt),
        ),
        (METHOD_API_KEY.into(), "Use an API key · pay per use".into()),
        (Row::Advanced.id(), "Add your own server · advanced".into()),
    ]
}

/// The id of the first step's "Use an API key" row.
pub const METHOD_API_KEY: &str = "apikey";

/// Login's second step: the providers an API key is for, then the servers
/// already configured, each with its state in a word or two.
pub fn provider_items(views: &[RowView]) -> Vec<(String, String)> {
    views
        .iter()
        .filter(|v| matches!(v.row, Row::AnthropicApi | Row::OpenaiApi | Row::Endpoint(_)))
        .map(|v| (v.row.id(), with_note(&v.row.name(), &v.state.short())))
        .collect()
}

/// The rows that signing out can remove something from, as picker items:
/// what `/login` can create (the ChatGPT sign-in, API keys, servers), also
/// when an environment variable is used instead. The gate's key is removed
/// in Settings or with `auth remove typesafe`.
pub fn logout_items(views: &[RowView]) -> Vec<(String, String)> {
    views
        .iter()
        .filter(|v| v.removable() && v.row != Row::Gate)
        .map(|v| (v.row.id(), with_note(&v.row.name(), &v.stored_label())))
        .collect()
}

/// The groups `auth status` prints the rows under.
fn status_group(row: &Row) -> &'static str {
    match row {
        Row::Chatgpt | Row::ClaudeCode | Row::CodexCli => "Plans you already pay for",
        Row::AnthropicApi | Row::OpenaiApi => "API keys · pay per use",
        Row::Endpoint(_) | Row::Advanced => "Your servers",
        Row::Gate => "Gate · decision service (not a model account)",
    }
}

/// `pathfinder auth status`: the rows under group headings, names in one
/// column and states (with detail: key hints, sources, model counts) after.
/// Nothing is cut: a long state wraps rather than loses its end.
pub fn status_lines(views: &[RowView]) -> Vec<String> {
    let shown: Vec<&RowView> = views.iter().filter(|v| v.row != Row::Advanced).collect();
    // The name column, as wide as the widest name up to a cap: a longer
    // name (a server's id) gets a line of its own, its state under it, so
    // it can't push every row past 80 columns.
    let col = shown
        .iter()
        .map(|v| v.row.name().chars().count())
        .filter(|&n| n <= STATUS_NAME_MAX)
        .max()
        .unwrap_or(0)
        + 2;
    let mut out = Vec::new();
    let mut group = "";
    for v in shown {
        let g = status_group(&v.row);
        if g != group {
            out.push(g.to_string());
            group = g;
        }
        let mut text = state_text(v);
        match (&v.row, &v.state) {
            (Row::Gate, State::NotSet) => text.push_str(" · `pathfinder data join`, or a key"),
            (Row::Endpoint(_), State::MissingKey) => {
                text.push_str(" · `pathfinder auth login` and pick it")
            }
            _ => {}
        }
        let name = v.row.name();
        if name.chars().count() > STATUS_NAME_MAX {
            out.push(format!("  {name}"));
            out.push(format!("  {:col$}{text}", ""));
        } else {
            out.push(format!("  {name:<col$}{text}"));
        }
    }
    out
}

/// The widest name `auth status` keeps in its name column.
const STATUS_NAME_MAX: usize = 20;

/// A list label with a state after the name: drawn as two columns.
pub fn with_note(name: &str, note: &str) -> String {
    crate::picker::with_note(name, note)
}

/// The gate's own key in a word or two, for its Settings row: from the
/// environment (which wins), stored, or not set.
pub fn gate_key_label(creds: &Credentials, env: Env) -> String {
    match key_state(
        creds,
        env,
        Some(Provider::Typesafe.key_env()),
        Provider::Typesafe.key_name(),
    ) {
        State::NotSet => "not set".into(),
        other => other.short(),
    }
}

#[cfg(test)]
mod tests {
    use std::sync::Arc;

    use super::super::secrets::MemoryStore;
    use super::*;
    use crate::auth::secrets::SecretStore;

    const SECRET: &str = "sk-ant-api03-VERYSECRETVALUE-xyzQAA";

    fn creds(store: Arc<MemoryStore>) -> Credentials {
        Credentials::for_tests(store, Arc::new(|_| Ok(None)))
    }

    fn no_env(_: &str) -> Option<String> {
        None
    }

    fn state(views: &[RowView], row: Row) -> State {
        views.iter().find(|v| v.row == row).unwrap().state.clone()
    }

    #[test]
    fn states_read_in_plain_words() {
        let store = Arc::new(MemoryStore::default());
        store.set("anthropic-api-key", SECRET).unwrap();
        let c = creds(store);
        let cfg = Config::default();
        let views = rows(
            &c,
            &cfg,
            CliAccounts {
                claude: true,
                codex: false,
            },
            &no_env,
        );
        assert_eq!(state(&views, Row::Chatgpt).label(), "not signed in");
        assert_eq!(
            state(&views, Row::ClaudeCode).label(),
            "installed · uses its own login"
        );
        assert_eq!(state(&views, Row::CodexCli).label(), "not installed");
        assert_eq!(
            state(&views, Row::AnthropicApi).label(),
            "key …zQAA · auth file"
        );
        assert_eq!(state(&views, Row::OpenaiApi).label(), "not configured");
        assert!(any_model_account(&views));
        assert_eq!(
            State::SignedIn {
                email: Some("a@b.c".into())
            }
            .label(),
            "signed in as a@b.c"
        );
    }

    #[test]
    fn a_tick_means_the_account_lists_models() {
        let key = |src| State::Key {
            hint: "…99ZZ".into(),
            source: src,
        };
        let env = KeySource::Env("ANTHROPIC_API_KEY");
        let view = |models| RowView {
            models,
            ..RowView::new(Row::AnthropicApi, key(env))
        };
        // Present but never checked, or refused: no tick.
        assert_eq!(state_text(&view(None)), "key …99ZZ · $ANTHROPIC_API_KEY");
        assert_eq!(
            state_text(&view(Some(0))),
            "key …99ZZ · $ANTHROPIC_API_KEY · no models listed"
        );
        assert_eq!(
            state_text(&view(Some(12))),
            "✓ key …99ZZ · $ANTHROPIC_API_KEY · 12 models"
        );
        // Counted from the catalog the pickers use.
        let entry = |id: &str, route| crate::config::ModelEntry {
            id: id.into(),
            label: id.into(),
            route,
            model: id.into(),
            effort: None,
            endpoint: None,
        };
        let counted = with_models(
            vec![
                RowView::new(Row::AnthropicApi, key(KeySource::File)),
                RowView::new(Row::OpenaiApi, State::NotSet),
                RowView::new(Row::Gate, State::NotSet),
            ],
            &[
                entry("anthropic:a", Route::Anthropic),
                entry("anthropic:b", Route::Anthropic),
            ],
        );
        assert_eq!(counted[0].models, Some(2));
        assert_eq!(counted[1].models, Some(0));
        assert_eq!(counted[2].models, None);
        assert_eq!(state_text(&counted[1]), "not configured");
    }

    #[test]
    fn an_environment_key_is_found_first_and_the_stored_one_stays_removable() {
        let store = Arc::new(MemoryStore::default());
        store
            .set("openai-api-key", "sk-proj-stored-0000000000")
            .unwrap();
        store
            .set("typesafe-api-key", "ts-stored-key-00001111")
            .unwrap();
        let c = creds(store);
        let env = |var: &str| (var == "OPENAI_API_KEY").then(|| "sk-proj-from-env-99999999".into());
        let cfg = Config {
            decisions: Some(crate::config::DecisionEndpoint {
                endpoint: "lab".into(),
                model: "qwen".into(),
            }),
            ..Config::default()
        };
        let views = rows(&c, &cfg, CliAccounts::default(), &env);
        let openai = views.iter().find(|v| v.row == Row::OpenaiApi).unwrap();
        assert_eq!(openai.state.label(), "key …9999 · $OPENAI_API_KEY");
        // The file's key is still there to remove, shown as itself.
        assert_eq!(openai.stored.as_deref(), Some("…0000"));
        assert!(openai.removable());
        // The gate uses an endpoint model, and the stored key still shows.
        let gate = views.iter().find(|v| v.row == Row::Gate).unwrap();
        assert!(matches!(gate.state, State::GateEndpoint { .. }));
        // Signing out lists what /login creates: not the gate's key (that
        // is in Settings and `auth remove typesafe`).
        let items = logout_items(&views);
        let ids: Vec<&str> = items.iter().map(|(id, _)| id.as_str()).collect();
        assert_eq!(ids, ["openai"]);
        assert_eq!(
            crate::picker::split_note(&items[0].1),
            ("OpenAI", "✓ stored")
        );
        // Only in the environment: nothing to remove.
        let env_only = rows(
            &creds(Arc::new(MemoryStore::default())),
            &Config::default(),
            CliAccounts::default(),
            &env,
        );
        assert!(logout_items(&env_only).is_empty());
        assert_eq!(Row::OpenaiApi.key_env(), Some("OPENAI_API_KEY"));
        assert_eq!(Row::Gate.key_env(), Some("TYPESAFE_API_KEY"));
        assert!(!any_model_account(&rows(
            &creds(Arc::new(MemoryStore::default())),
            &Config::default(),
            CliAccounts::default(),
            &no_env
        )));
    }

    #[test]
    fn keys_never_appear_in_lines_or_status() {
        let store = Arc::new(MemoryStore::default());
        store.set("anthropic-api-key", SECRET).unwrap();
        store.set("typesafe-api-key", SECRET).unwrap();
        let c = creds(store);
        let views = rows(&c, &Config::default(), CliAccounts::default(), &no_env);
        let labels = |items: Vec<(String, String)>| -> Vec<String> {
            items.into_iter().map(|(_, l)| l).collect()
        };
        let all = [
            labels(method_items(&views)),
            labels(provider_items(&views)),
            status_lines(&views),
            labels(logout_items(&views)),
        ]
        .concat()
        .join("\n");
        assert!(!all.contains("VERYSECRET"), "{all}");
        assert!(!all.contains("sk-ant"), "{all}");
        assert_eq!(logout_items(&views).len(), 1);
        // Nor when pasted where the account name goes.
        let said = unknown_account(SECRET);
        assert!(!said.contains("VERYSECRET"), "{said}");
        assert!(said.contains("…zQAA"), "{said}");
        assert_eq!(
            unknown_account("nope"),
            format!("No account `nope`. Use {ALL_NAMES}.")
        );
        assert!(!looks_like_key("chatgpt"));
    }

    #[test]
    fn login_lists_use_short_names_and_short_states() {
        let store = Arc::new(MemoryStore::default());
        store.set("openai-api-key", SECRET).unwrap();
        let c = creds(store);
        let env = |v: &str| (v == "ANTHROPIC_API_KEY").then(|| "sk-ant-from-env-0000".into());
        let cfg = Config {
            endpoints: vec![crate::config::Endpoint {
                id: "lab".into(),
                label: None,
                base_url: "http://127.0.0.1:8000/v1".into(),
                key: None,
                models: Vec::new(),
                extra: None,
            }],
            ..Config::default()
        };
        let views = rows(
            &c,
            &cfg,
            CliAccounts {
                claude: true,
                codex: true,
            },
            &env,
        );
        let split = |items: Vec<(String, String)>| -> Vec<(String, String, String)> {
            items
                .into_iter()
                .map(|(id, l)| {
                    let (name, note) = crate::picker::split_note(&l);
                    (id, name.to_string(), note.to_string())
                })
                .collect()
        };
        // Step one: how to connect; who pays is said here, once.
        let methods = split(method_items(&views));
        let ids: Vec<&str> = methods.iter().map(|m| m.0.as_str()).collect();
        assert_eq!(ids, ["chatgpt", METHOD_API_KEY, "endpoint"]);
        assert_eq!(methods[0].1, "Sign in with your ChatGPT plan");
        assert_eq!(methods[0].2, "not signed in");
        assert_eq!(methods[1].1, "Use an API key · pay per use");
        assert_eq!(methods[2].1, "Add your own server · advanced");
        // No CLI rows: Pathfinder never signs in to those.
        assert!(!format!("{methods:?}").contains("Claude Code"));
        // Step two: short names, a word or two of state.
        let providers = split(provider_items(&views));
        assert_eq!(
            providers,
            [
                (
                    "anthropic".into(),
                    "Anthropic".into(),
                    "✓ from $ANTHROPIC_API_KEY".into()
                ),
                ("openai".into(), "OpenAI".into(), "✓ stored".into()),
                ("endpoint:lab".into(), "lab".into(), "no key needed".into()),
            ]
        );
        let all = format!("{providers:?}");
        for never in ["…", "pay per use", "auth file", "model"] {
            assert!(!all.contains(never), "{never}: {all}");
        }
        assert_eq!(State::NotSet.short(), "not configured");
        assert_eq!(
            State::SignedIn {
                email: Some("a@b.c".into())
            }
            .short(),
            "✓ signed in as a@b.c"
        );
        // The gate key's Settings row.
        assert_eq!(gate_key_label(&c, &no_env), "not set");
        let gate_env = |v: &str| (v == "TYPESAFE_API_KEY").then(|| "ts-from-env-0000".into());
        assert_eq!(gate_key_label(&c, &gate_env), "✓ from $TYPESAFE_API_KEY");
    }

    #[test]
    fn status_groups_rows_under_headings_with_detail() {
        let store = Arc::new(MemoryStore::default());
        store.set("anthropic-api-key", SECRET).unwrap();
        let c = creds(store);
        let views = rows(&c, &Config::default(), CliAccounts::default(), &no_env);
        let lines = status_lines(&views);
        assert_eq!(lines[0], "Plans you already pay for");
        assert!(lines[1].starts_with("  ChatGPT plan  "), "{lines:?}");
        let keys = lines
            .iter()
            .position(|l| l == "API keys · pay per use")
            .unwrap();
        assert!(lines[keys + 1].starts_with("  Anthropic "), "{lines:?}");
        assert!(
            lines[keys + 1].ends_with("key …zQAA · auth file"),
            "{lines:?}"
        );
        assert!(
            lines
                .iter()
                .any(|l| l.starts_with("Gate · decision service"))
        );
        // "pay per use" once, as a heading, never per row.
        assert_eq!(lines.join("\n").matches("pay per use").count(), 1);
        // States line up in one column; nothing is cut.
        let columns: Vec<usize> = lines
            .iter()
            .filter(|l| l.starts_with("  "))
            .map(|l| {
                let chars: Vec<char> = l.chars().collect();
                let gap = 2 + chars[2..].windows(2).position(|w| w == [' ', ' ']).unwrap();
                gap + chars[gap..].iter().take_while(|c| **c == ' ').count()
            })
            .collect();
        assert!(columns.iter().all(|c| *c == columns[0]), "{lines:?}");
        assert!(lines.iter().all(|l| l.chars().count() <= 80), "{lines:?}");
    }

    #[test]
    fn a_long_server_name_gets_its_own_status_line_and_keeps_80_columns() {
        let cfg = Config {
            endpoints: vec![crate::config::Endpoint {
                id: "very-long-server-name-for-overflow-testing".into(),
                label: None,
                base_url: "http://localhost:11434/v1".into(),
                key: None,
                models: Vec::new(),
                extra: None,
            }],
            ..Config::default()
        };
        let views = rows(
            &creds(Arc::new(MemoryStore::default())),
            &cfg,
            CliAccounts {
                claude: true,
                codex: true,
            },
            &no_env,
        );
        let views = with_models(views, &[]);
        let lines = status_lines(&views);
        assert!(lines.iter().all(|l| l.chars().count() <= 80), "{lines:#?}");
        let at = lines
            .iter()
            .position(|l| l == "  very-long-server-name-for-overflow-testing")
            .unwrap();
        assert!(lines[at + 1].trim_start().starts_with("no key needed"));
        // The others keep a column sized to their own names.
        assert!(lines[1].starts_with("  ChatGPT plan  "), "{lines:?}");
    }

    #[test]
    fn names_parse() {
        let cfg = Config::default();
        for (name, row) in [
            ("chatgpt", Row::Chatgpt),
            ("anthropic", Row::AnthropicApi),
            ("openai", Row::OpenaiApi),
            ("endpoint", Row::Advanced),
            ("server", Row::Advanced),
            ("typesafe", Row::Gate),
            ("gate", Row::Gate),
            ("claude", Row::ClaudeCode),
        ] {
            assert_eq!(parse_account(name, &cfg), Some(row), "{name}");
        }
        assert_eq!(parse_account("nope", &cfg), None);
        assert_eq!(parse_account("endpoint:lab", &cfg), None);
        for row in [Row::Chatgpt, Row::Gate, Row::Endpoint("lab".into())] {
            assert_eq!(Row::from_id(&row.id()), Some(row));
        }
        assert!(
            Row::ClaudeCode
                .sign_in_hint(true)
                .unwrap()
                .contains("/login")
        );
        assert!(
            Row::CodexCli
                .sign_in_hint(false)
                .unwrap()
                .contains("isn't installed")
        );
    }
}
