//! Small, shared helpers over an MDX `WHERE`-clause slicer's resolved
//! members — used by `daxgen` (DAX generation) and by `server::xmla`'s
//! Multidimensional adapter (which needs the same measure-name resolution
//! `daxgen` used, to read the matching `[M0]`/`[M1]` DAX result column when
//! no axis carries the Measures hierarchy).

use super::eval::Member;

pub fn slicer_measure_names(slicer: &[Member]) -> Result<Vec<String>, String> {
    Ok(slicer
        .iter()
        .filter_map(|m| match m {
            Member::Measure { name } => Some(name.clone()),
            Member::Leaf { .. } | Member::All { .. } => None,
        })
        .collect())
}
