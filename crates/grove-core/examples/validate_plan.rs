//! Validate a native plan and its local assets using the production importer.
fn main() -> Result<(), String> {
    let input = std::env::args()
        .nth(1)
        .ok_or("Usage: validate_plan ABSOLUTE_PLAN_PATH")?;
    let path = std::path::Path::new(&input)
        .canonicalize()
        .map_err(|e| e.to_string())?;
    let root = path.parent().ok_or("Plan has no parent directory")?;
    let mut store = grove_core::rich_plan::PlanStore::default();
    store.import(
        "validation",
        path.to_str().ok_or("Path is not UTF-8")?,
        root.to_str().ok_or("Root is not UTF-8")?,
        &[root.to_owned()],
    )?;
    let plan = &store.revisions[0];
    println!(
        "PASS schema v{}: {} · revision {} · {} snapshotted assets",
        plan.document.schema_version,
        plan.document.title,
        plan.document.revision_id,
        plan.assets.len()
    );
    Ok(())
}
