use std::collections::{HashMap, HashSet, VecDeque};
use std::path::PathBuf;

use serde::{Deserialize, Serialize};
use thiserror::Error;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum SessionMode {
    Onboarding,
    Working,
    Done,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ItemStatus {
    Pending,
    Working,
    Understood,
    HandedOff,
    Skipped,
}

impl ItemStatus {
    pub fn is_resolved(self) -> bool {
        matches!(self, Self::Understood | Self::HandedOff | Self::Skipped)
    }

    pub fn symbol(self) -> &'static str {
        match self {
            Self::Pending => "[ ]",
            Self::Working => "[>]",
            Self::Understood => "[x]",
            Self::HandedOff => "[~]",
            Self::Skipped => "[-]",
        }
    }

    pub fn label(self) -> &'static str {
        match self {
            Self::Pending => "pending",
            Self::Working => "working",
            Self::Understood => "understood",
            Self::HandedOff => "handed_off",
            Self::Skipped => "skipped",
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct PathItem {
    pub id: String,
    pub title: String,
    #[serde(default)]
    pub depends_on: Vec<String>,
    pub user_task: String,
    pub check_criteria: String,
    pub status: ItemStatus,
    #[serde(default)]
    pub notes: Option<String>,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize, PartialEq, Eq)]
pub struct PlanGraph {
    pub items: Vec<PathItem>,
}

#[derive(Debug, Error, PartialEq, Eq)]
pub enum GraphError {
    #[error("plan must contain at least one item")]
    Empty,
    #[error("duplicate item id `{0}`")]
    DuplicateId(String),
    #[error("item `{item}` depends on unknown id `{missing}`")]
    MissingDependency { item: String, missing: String },
    #[error("item `{0}` cannot depend on itself")]
    SelfDependency(String),
    #[error("plan contains a dependency cycle")]
    Cycle,
}

impl PlanGraph {
    pub fn validate(&self) -> Result<(), GraphError> {
        if self.items.is_empty() {
            return Err(GraphError::Empty);
        }

        let mut ids = HashSet::with_capacity(self.items.len());
        for item in &self.items {
            if !ids.insert(item.id.as_str()) {
                return Err(GraphError::DuplicateId(item.id.clone()));
            }
        }

        let mut indegree: HashMap<&str, usize> =
            self.items.iter().map(|i| (i.id.as_str(), 0)).collect();
        let mut dependents: HashMap<&str, Vec<&str>> = HashMap::new();

        for item in &self.items {
            for dep in &item.depends_on {
                if dep == &item.id {
                    return Err(GraphError::SelfDependency(item.id.clone()));
                }
                if !ids.contains(dep.as_str()) {
                    return Err(GraphError::MissingDependency {
                        item: item.id.clone(),
                        missing: dep.clone(),
                    });
                }
                *indegree.get_mut(item.id.as_str()).unwrap() += 1;
                dependents
                    .entry(dep.as_str())
                    .or_default()
                    .push(item.id.as_str());
            }
        }

        let mut queue: VecDeque<&str> = indegree
            .iter()
            .filter_map(|(&id, &deg)| (deg == 0).then_some(id))
            .collect();
        let mut visited = 0usize;

        while let Some(id) = queue.pop_front() {
            visited += 1;
            if let Some(next_ids) = dependents.get(id) {
                for &next in next_ids {
                    let deg = indegree.get_mut(next).unwrap();
                    *deg -= 1;
                    if *deg == 0 {
                        queue.push_back(next);
                    }
                }
            }
        }

        if visited != self.items.len() {
            return Err(GraphError::Cycle);
        }

        Ok(())
    }

    pub fn get(&self, id: &str) -> Option<&PathItem> {
        self.items.iter().find(|i| i.id == id)
    }

    pub fn get_mut(&mut self, id: &str) -> Option<&mut PathItem> {
        self.items.iter_mut().find(|i| i.id == id)
    }

    pub fn deps_satisfied(&self, item: &PathItem) -> bool {
        item.depends_on.iter().all(|dep| {
            self.get(dep)
                .map(|d| d.status.is_resolved())
                .unwrap_or(false)
        })
    }

    pub fn next_ready_id(&self) -> Option<String> {
        self.items
            .iter()
            .find(|item| !item.status.is_resolved() && self.deps_satisfied(item))
            .map(|item| item.id.clone())
    }

    pub fn all_resolved(&self) -> bool {
        !self.items.is_empty() && self.items.iter().all(|i| i.status.is_resolved())
    }

    pub fn render_map(&self, active_id: Option<&str>) -> String {
        if self.items.is_empty() {
            return "No path built yet.".to_string();
        }
        let mut lines = Vec::with_capacity(self.items.len() + 2);
        lines.push("Path map (`[x]` understood, `[>]` active, `[~]` handed off, `[-]` skipped, `[ ]` pending):".to_string());
        for item in &self.items {
            let marker = if Some(item.id.as_str()) == active_id && !item.status.is_resolved() {
                "[>]"
            } else {
                item.status.symbol()
            };
            let deps = if item.depends_on.is_empty() {
                String::new()
            } else {
                format!("  <- {}", item.depends_on.join(", "))
            };
            lines.push(format!("- `{}` {} {}{}", item.id, marker, item.title, deps));
        }
        lines.join("\n")
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TurnRecord {
    pub role: String,
    pub content: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Session {
    pub id: String,
    pub cwd: PathBuf,
    pub goal: Option<String>,
    pub starting_point: Option<String>,
    pub mode: SessionMode,
    pub plan: PlanGraph,
    pub active_item: Option<String>,
    pub turns_on_active: u32,
    pub history: Vec<TurnRecord>,
}

impl Session {
    pub fn new(id: String, cwd: PathBuf) -> Self {
        Self {
            id,
            cwd,
            goal: None,
            starting_point: None,
            mode: SessionMode::Onboarding,
            plan: PlanGraph::default(),
            active_item: None,
            turns_on_active: 0,
            history: Vec::new(),
        }
    }

    pub fn push_turn(&mut self, role: &str, content: impl Into<String>) {
        self.history.push(TurnRecord {
            role: role.to_string(),
            content: content.into(),
        });
        if self.history.len() > 40 {
            let drain = self.history.len() - 40;
            self.history.drain(0..drain);
        }
    }

    pub fn active(&self) -> Option<&PathItem> {
        let id = self.active_item.as_deref()?;
        self.plan.get(id)
    }

    pub fn advance_active(&mut self) -> Option<String> {
        let next = self.plan.next_ready_id();
        self.active_item = next.clone();
        self.turns_on_active = 0;
        if let Some(ref id) = next {
            if let Some(item) = self.plan.get_mut(id) {
                if item.status == ItemStatus::Pending {
                    item.status = ItemStatus::Working;
                }
            }
            self.mode = SessionMode::Working;
        } else if self.plan.all_resolved() {
            self.mode = SessionMode::Done;
        }
        next
    }
}

// --- Type-safe structured model outputs ---

#[derive(Debug, Clone, Deserialize)]
pub struct OnboardingDecision {
    pub reply: String,
    pub ready_to_plan: bool,
    #[serde(default)]
    pub goal_summary: Option<String>,
    #[serde(default)]
    pub starting_point: Option<String>,
}

#[derive(Debug, Clone, Deserialize)]
pub struct ProposedItem {
    pub id: String,
    pub title: String,
    #[serde(default)]
    pub depends_on: Vec<String>,
    pub user_task: String,
    pub check_criteria: String,
}

#[derive(Debug, Clone, Deserialize)]
pub struct PlanProposal {
    pub brief: String,
    pub items: Vec<ProposedItem>,
}

impl PlanProposal {
    pub fn into_graph(self) -> Result<(String, PlanGraph), GraphError> {
        let graph = PlanGraph {
            items: self
                .items
                .into_iter()
                .map(|p| PathItem {
                    id: p.id,
                    title: p.title,
                    depends_on: p.depends_on,
                    user_task: p.user_task,
                    check_criteria: p.check_criteria,
                    status: ItemStatus::Pending,
                    notes: None,
                })
                .collect(),
        };
        graph.validate()?;
        Ok((self.brief, graph))
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum CoachAction {
    KeepGoing,
    SuggestCheck,
    Escalate,
    NeedReplan,
}

#[derive(Debug, Clone, Deserialize)]
pub struct CoachDecision {
    pub reply: String,
    pub action: CoachAction,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum CheckVerdict {
    Pass,
    Close,
    NotYet,
}

#[derive(Debug, Clone, Deserialize)]
pub struct CheckDecision {
    pub verdict: CheckVerdict,
    pub feedback: String,
}

#[cfg(test)]
mod tests {
    use super::*;

    fn item(id: &str, deps: &[&str], status: ItemStatus) -> PathItem {
        PathItem {
            id: id.to_string(),
            title: format!("Title {id}"),
            depends_on: deps.iter().map(|s| s.to_string()).collect(),
            user_task: "Do the work".to_string(),
            check_criteria: "Explain why it works".to_string(),
            status,
            notes: None,
        }
    }

    #[test]
    fn validates_dag_and_picks_next_ready() {
        let graph = PlanGraph {
            items: vec![
                item("a", &[], ItemStatus::Understood),
                item("b", &["a"], ItemStatus::Pending),
                item("c", &["b"], ItemStatus::Pending),
            ],
        };
        assert_eq!(graph.validate(), Ok(()));
        assert_eq!(graph.next_ready_id().as_deref(), Some("b"));
    }

    #[test]
    fn rejects_cycle() {
        let graph = PlanGraph {
            items: vec![
                item("a", &["b"], ItemStatus::Pending),
                item("b", &["a"], ItemStatus::Pending),
            ],
        };
        assert_eq!(graph.validate(), Err(GraphError::Cycle));
    }

    #[test]
    fn rejects_missing_dependency() {
        let graph = PlanGraph {
            items: vec![item("a", &["ghost"], ItemStatus::Pending)],
        };
        assert_eq!(
            graph.validate(),
            Err(GraphError::MissingDependency {
                item: "a".to_string(),
                missing: "ghost".to_string(),
            })
        );
    }

    #[test]
    fn rejects_duplicate_id() {
        let graph = PlanGraph {
            items: vec![
                item("a", &[], ItemStatus::Pending),
                item("a", &[], ItemStatus::Pending),
            ],
        };
        assert_eq!(graph.validate(), Err(GraphError::DuplicateId("a".into())));
    }
}
