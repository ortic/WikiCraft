use wikicraft_confluence::{PlanOptions, Planner, Stats};
fn main() {
    let path = std::env::args().nth(1).unwrap();
    let export = wikicraft_confluence::open(std::path::Path::new(&path)).unwrap();
    let planner = Planner::new(&export, PlanOptions::default());
    let plan = planner.plan();
    let mut cur = Stats::default();
    let mut hist = Stats::default();
    for s in &plan.spaces {
        for p in &s.pages {
            planner.convert(&p.source_id, &mut cur);
            for h in &p.history {
                planner.convert(&h.source_id, &mut hist);
            }
        }
    }
    println!(
        "current: {}/{} missing samples {:?}",
        cur.page_links_resolved,
        cur.page_links,
        &cur.missing_link_samples[..cur.missing_link_samples.len().min(8)]
    );
    println!(
        "history: {}/{} missing samples {:?}",
        hist.page_links_resolved,
        hist.page_links,
        &hist.missing_link_samples[..hist.missing_link_samples.len().min(8)]
    );
    let titles: Vec<_> = plan.spaces[0].pages.iter().map(|p| p.title.clone()).take(50).collect();
    println!("titles: {titles:?}");
}
