//! Running a planned workflow: the store, the call cache, budgets, the retry owner, the agent loop,
//! the event log, run folders, and the node hosts.
//!
//! | module | owns |
//! |---|---|
//! | [`engine`] | what a command shares: the runtime handle, the store, the host pool, adapters; one invocation's services; cancellation |
//! | [`store`] | spec/store.md §1–§7: files, result/call/job records, trust, atomic writes, read sets |
//! | [`events`] | `events.jsonl`: the fx-run-events-v1 writer and readers, value encoding |
//! | [`folder`] | run folders: names, `plan.json`, `run.lock`, placing files |
//! | [`ledger`] | the ceiling, step budgets, holds and their settlement |
//! | [`calls`] | the `capability` request path, the retry owner, route pacing |
//! | [`stand_in`] | stand-in runs: the answerer of a stand-in's calls, the checks its answers meet |
//! | [`agent`] | the agent loop (`agent.run`) |
//! | [`executor`] | one attempt of one instance: result cache, select, paid built-ins, host runs, accepting results |
//! | [`runner`] | one run invocation: refusals, resume, scheduling, dispatch, outputs |
//! | [`plantime`] | `at: plan` steps while planning |
//! | [`host`] | node host processes, their connections and pool; the planning host |
//! | [`tools`] | external programs (`GRIDA_FX_TOOL_<NAME>`, then `PATH`) |
//!
//! The data flow, the threads and the attempt/billing state machine are in the step-3
//! ARCHITECTURE notes; each module's doc names the spec sections it follows.

pub mod agent;
pub mod calls;
pub mod engine;
pub mod events;
pub mod executor;
pub mod folder;
pub mod host;
pub mod ledger;
pub mod plantime;
pub mod runner;
pub mod stand_in;
pub mod store;
pub mod tools;
