use std::env;
use std::fs;

use serde_json::json;
use wakaru_core::scope_hoist::trace_scope_hoisted;

fn main() -> Result<(), Box<dyn std::error::Error>> {
    let path = env::args()
        .nth(1)
        .ok_or("usage: scope_hoist_trace <input.js>")?;
    let source = fs::read_to_string(&path)?;
    let trace = trace_scope_hoisted(&source).ok_or("failed to parse input as an ES module")?;

    let payload = json!({
        "input": path,
        "source_bytes": trace.source_bytes,
        "minimum_declarations": trace.minimum_declarations,
        "declaration_count": trace.declaration_count,
        "eligible": trace.eligible,
        "would_split": trace.would_split,
        "signal_cluster_count": trace.signal_cluster_count,
        "post_write_cluster_count": trace.post_write_cluster_count,
        "component_cap_output_cluster_count": trace.component_cap_output_cluster_count,
        "leaf_candidate_output_cluster_count": trace.leaf_candidate_output_cluster_count,
        "bounded_leaf_restoration_accepted": trace.bounded_leaf_restoration_accepted,
        "items": trace.items.into_iter().map(|item| json!({
            "index": item.index,
            "source_range": item.source_range,
            "declared_names": item.declared_names,
            "referenced_items": item.referenced_items,
            "written_items": item.written_items,
            "signal_cluster": item.signal_cluster,
            "post_write_cluster": item.post_write_cluster,
        })).collect::<Vec<_>>(),
        "cross_write_edges": trace.cross_write_edges.into_iter().map(|edge| json!({
            "writer_item": edge.writer_item,
            "owner_item": edge.owner_item,
            "writer_cluster": edge.writer_cluster,
            "owner_cluster": edge.owner_cluster,
            "writer_target_cluster_degree": edge.writer_target_cluster_degree,
            "component_cluster_count": edge.component_cluster_count,
            "leaf_component_cluster_count": edge.leaf_component_cluster_count,
            "kept_by_inspect_policy": edge.kept_by_inspect_policy,
        })).collect::<Vec<_>>(),
    });
    println!("{}", serde_json::to_string_pretty(&payload)?);
    Ok(())
}
