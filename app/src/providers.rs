//! The model providers' public API addresses. Each can be overridden with a
//! `PATHFINDER_<PROVIDER>_BASE_URL` variable where it's used.

pub const OPENAI: &str = "https://api.openai.com/v1";
pub const ANTHROPIC: &str = "https://api.anthropic.com";
pub const GEMINI: &str = "https://generativelanguage.googleapis.com/v1beta";
pub const TYPESAFE: &str = "https://api.typesafe.ai/v1";
