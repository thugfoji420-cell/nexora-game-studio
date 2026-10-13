// Reuse production code and its tests, rather than duplicating the security rules.
// This target links only the filesystem dependencies used by this module.
#[path = "../src/workspace_files.rs"]
mod workspace_files;

#[path = "../src/code_agent_protocol.rs"]
mod code_agent_protocol;
