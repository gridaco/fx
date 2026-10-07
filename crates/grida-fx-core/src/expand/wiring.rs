//! Display-only references observed during evaluation. These never participate in a value,
//! dependency, identity or scheduling decision. Unresolved selections have no invented port.

use std::collections::{BTreeMap, BTreeSet};

use serde::Serialize;

/// A resolved output or fact read by one `with` entry. Several references may feed one entry.
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Serialize)]
pub struct Binding {
    pub source: String,
    pub source_port: String,
    pub target_port: String,
    pub source_kind: SourceKind,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum SourceKind {
    Output,
    Fact,
    ScopeInput,
    ScopeOutput,
}

#[derive(Debug, Clone)]
enum Source {
    /// The object at `steps.x.outputs` or `steps.x.facts`, before its member is read.
    Result {
        id: String,
        kind: SourceKind,
        ports: Vec<String>,
        any_port: bool,
    },
    /// A used workflow's output aliases, evaluated without leaking its other outputs.
    Aliases {
        bindings: BTreeMap<String, Vec<Binding>>,
        interfaces: BTreeMap<String, Vec<Binding>>,
    },
}

#[derive(Debug, Clone, Default)]
struct Access {
    capture: Option<usize>,
    sources: Vec<Source>,
}

#[derive(Debug, Clone)]
struct Capture {
    target: String,
    bindings: BTreeSet<Binding>,
    interfaces: BTreeSet<Binding>,
}

/// Independent from the legacy read sets, which deliberately include nested expansion reads.
#[derive(Debug, Clone, Default)]
pub(crate) struct Trace {
    captures: Vec<Option<Capture>>,
    accesses: Vec<Access>,
}

impl Trace {
    pub fn begin_capture(&mut self, target: &str) {
        self.captures.push(Some(Capture {
            target: target.into(),
            bindings: BTreeSet::new(),
            interfaces: BTreeSet::new(),
        }));
    }

    /// Flattened leaf lineage and exact interface lineage from the same evaluation.
    pub fn end_capture_full(&mut self) -> (Vec<Binding>, Vec<Binding>) {
        self.captures
            .pop()
            .flatten()
            .map(|capture| {
                (
                    capture.bindings.into_iter().collect(),
                    capture.interfaces.into_iter().collect(),
                )
            })
            .unwrap_or_default()
    }

    /// Expanding a referenced step must not attribute its own inputs to its reader.
    pub fn suspend(&mut self) {
        self.captures.push(None);
    }

    pub fn resume(&mut self) {
        self.captures.pop();
    }

    fn current_capture(&self) -> Option<usize> {
        self.captures
            .len()
            .checked_sub(1)
            .filter(|&index| self.captures[index].is_some())
    }

    pub fn begin_access(&mut self) {
        self.accesses.push(Access {
            capture: self.current_capture(),
            sources: Vec::new(),
        });
    }

    pub fn end_access(&mut self, ok: bool) {
        let Some(access) = self.accesses.pop() else {
            return;
        };
        if !ok {
            return;
        }
        // `let.alias.image`: an alias of the whole outputs object keeps its origin until
        // the surrounding access picks the field, rather than claiming every output.
        if let Some(outer) = self.accesses.last_mut()
            && outer.capture == access.capture
        {
            outer.sources.extend(access.sources);
            return;
        }
        for source in access.sources {
            self.emit(access.capture, source, None);
        }
    }

    pub fn result(&mut self, id: &str, kind: SourceKind, ports: Vec<String>, any_port: bool) {
        let current = self.current_capture();
        if let Some(access) = self.accesses.last_mut()
            && access.capture == current
        {
            access.sources.push(Source::Result {
                id: id.into(),
                kind,
                ports,
                any_port,
            });
        }
    }

    pub fn aliases(&mut self, aliases: BTreeMap<String, Vec<Binding>>) {
        self.alias_sources(aliases.clone(), aliases);
    }

    /// Preserve the public boundary name independently from its flattened leaf lineage.
    pub fn boundary(
        &mut self,
        aliases: BTreeMap<String, Vec<Binding>>,
        scope: &str,
        kind: SourceKind,
        ports: impl IntoIterator<Item = String>,
    ) {
        let interfaces = ports
            .into_iter()
            .map(|port| {
                (
                    port.clone(),
                    vec![Binding {
                        source: scope.into(),
                        source_port: port.clone(),
                        source_kind: kind,
                        target_port: port,
                    }],
                )
            })
            .collect();
        self.alias_sources(aliases, interfaces);
    }

    fn alias_sources(
        &mut self,
        bindings: BTreeMap<String, Vec<Binding>>,
        interfaces: BTreeMap<String, Vec<Binding>>,
    ) {
        let capture = self.current_capture();
        let source = Source::Aliases {
            bindings,
            interfaces,
        };
        if let Some(access) = self.accesses.last_mut()
            && access.capture == capture
        {
            access.sources.push(source);
        } else {
            self.emit(capture, source, None);
        }
    }

    pub fn member(&mut self, name: &str) {
        let Some(access) = self.accesses.last_mut() else {
            return;
        };
        let capture = access.capture;
        let sources = std::mem::take(&mut access.sources);
        for source in sources {
            self.emit(capture, source, Some(name));
        }
    }

    pub fn unknown_member(&mut self) {
        if let Some(access) = self.accesses.last_mut() {
            access.sources.clear();
        }
    }

    pub fn references(&mut self, bindings: &[Binding], interfaces: &[Binding]) {
        let Some(capture) = self.captures.last_mut().and_then(Option::as_mut) else {
            return;
        };
        for (target, values) in [
            (&mut capture.bindings, bindings),
            (&mut capture.interfaces, interfaces),
        ] {
            target.extend(values.iter().cloned().map(|mut binding| {
                binding.target_port.clone_from(&capture.target);
                binding
            }));
        }
    }

    fn emit(&mut self, index: Option<usize>, source: Source, member: Option<&str>) {
        let Some(capture) = index
            .and_then(|index| self.captures.get_mut(index))
            .and_then(Option::as_mut)
        else {
            return;
        };
        match source {
            Source::Result {
                id,
                kind,
                ports,
                any_port,
            } => {
                let selected = match member {
                    Some(_) if any_port => vec!["value".to_string()],
                    Some(name) if kind == SourceKind::Fact || ports.iter().any(|p| p == name) => {
                        vec![name.to_string()]
                    }
                    Some(_) => Vec::new(),
                    None => ports,
                };
                let bindings: Vec<_> = selected
                    .into_iter()
                    .map(|source_port| Binding {
                        source: id.clone(),
                        source_port,
                        target_port: capture.target.clone(),
                        source_kind: kind,
                    })
                    .collect();
                capture.bindings.extend(bindings.iter().cloned());
                capture.interfaces.extend(bindings);
            }
            Source::Aliases {
                bindings,
                interfaces,
            } => {
                for (target, aliases) in [
                    (&mut capture.bindings, bindings),
                    (&mut capture.interfaces, interfaces),
                ] {
                    let selected: Vec<Binding> = match member {
                        Some(name) => aliases.get(name).cloned().unwrap_or_default(),
                        None => aliases.into_values().flatten().collect(),
                    };
                    target.extend(selected.into_iter().map(|mut binding| {
                        binding.target_port.clone_from(&capture.target);
                        binding
                    }));
                }
            }
        }
    }
}
