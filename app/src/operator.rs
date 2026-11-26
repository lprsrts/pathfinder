//! Who runs the services this build talks to. A fork that runs its own
//! collector changes this file (or sets `PATHFINDER_COLLECTOR`).

/// The collector: usage data, sign-in, the shared gate, releases.
pub const COLLECTOR: &str = "https://pathfinder.lprsrts.com";

/// Where bugs are reported in public.
pub const ISSUES: &str = "https://github.com/lprsrts/pathfinder/issues";

/// Who to write to about this build.
pub const CONTACT: &str = "alper@saritas.io";

/// Who runs the collector, as the people they invite know them.
pub const NAME: &str = "Alper";
