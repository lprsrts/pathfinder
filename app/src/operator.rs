//! Who runs the services this build talks to. A fork that runs its own
//! collector changes this file (or sets `PATHFINDER_COLLECTOR`).

/// The collector: usage data, sign-in, the shared gate, releases.
pub const COLLECTOR: &str = "https://pathfinder.lprsrts.com";

/// A second home for the same signed release files, used when the
/// collector can't be reached.
pub const RELEASES_MIRROR: &str = "https://github.com/lprsrts/pathfinder/releases";

/// Where bugs are reported in public.
pub const ISSUES: &str = "https://github.com/lprsrts/pathfinder/issues";

/// Who to write to about this build.
pub const CONTACT: &str = "alper@saritas.io";

/// Who runs the collector, as the people they invite know them.
pub const NAME: &str = "Alper";
