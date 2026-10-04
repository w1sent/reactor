//! Taking the graph to other tools, and back
//! ([ADR-0047](../../../../docs/adr/0047-the-analysis-graph-is-the-case-s-model.md)).
//!
//! * [`stix`] — a STIX 2.1 bundle, for sharing finished intelligence. It is a mapping, not
//!   the model: STIX has no place for versions, record time apart from domain time, or a
//!   claim being questioned, so each object also carries what REactor knows in an
//!   `x_reactor` property, and importing a bundle that has it restores the graph exactly.
//! * [`tables`] — elements and edges as CSV, for a spreadsheet or a script.
//! * [`projection`] — the entities joined by the claims they share, as an edge list or
//!   GraphML, for graph libraries (NetworkX, igraph, Gephi) that need an entity-to-entity
//!   graph. Built per question and never stored.

pub mod projection;
pub mod stix;
pub mod tables;
