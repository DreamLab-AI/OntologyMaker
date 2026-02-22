pub mod defs;
pub mod file_ops;
pub mod patch;
pub mod policy;
pub mod search;
pub mod shell;
pub mod web;
pub mod workspace;

// Primary public API
pub use defs::{get_tool_definitions, to_anthropic_tools, to_openai_tools, TOOL_DEFINITIONS};
pub use workspace::WorkspaceTools;

// Commonly used types from submodules
pub use patch::{ApplyReport, HashlineOp, PatchOp};
pub use policy::{check_shell_policy, ExecutionScope, WriteConflictTracker};
pub use search::Symbol;
pub use shell::BgJobManager;
pub use web::ExaClient;
