//! Compile gate for the include-fetch shapes behind issues #458/#459
//! (`include_shapes_generated_code_compiles`).
//!
//! Fixture (`tests/fixtures2/`): a scalar `refers` edge (PositionType →
//! JobType), array `$ref` cross-references from DeploymentType, and decoy
//! properties on the targets — an array-of-strings property NAMED
//! `deployment` on JobType and an unformatted TEXT column `deployment_id`
//! on JobType/WorkerType. Before the fixes the reverse-FK resolution
//! accepted those decoys as genuine back-FKs and the repository emitter
//! filtered `core_job::Column::Deployment` (E0599 — no such variant) and
//! keyed the batch map on `row.deployment_id: Option<String>` against a
//! `Uuid` key (E0308). The generated app must `cargo check` cleanly.

use std::path::Path;
use std::time::Duration;

use crate::setup::{count_files_named, generate_full_app_from};

async fn check_include_shapes_app() {
    let tmp = tempfile::tempdir().unwrap();
    let output_dir = tmp.path();

    generate_full_app_from(output_dir, Path::new("tests/fixtures2")).await;

    let repository_impl_count =
        count_files_named(&output_dir.join("src").join("domain"), "repository_impl.rs");
    assert!(
        repository_impl_count > 0,
        "include-shapes gate must emit repository_impl.rs files"
    );

    // The genuine reverse includes resolve child-side: deployments-for-job
    // scans the CHILD table by its own FK column (the issue's Expected), and
    // the wrong-side decoy fetches are gone entirely.
    let job_repo =
        std::fs::read_to_string(output_dir.join("src/domain/core/job/repository_impl.rs")).unwrap();
    assert!(
        job_repo.contains("crate::entity::core_deployment::Column::JobId.eq(source_id)"),
        "deployments-for-job must filter the child (deployment) table by its own \
         job_id FK column. Got:\n{job_repo}"
    );
    assert!(
        !job_repo.contains("Column::Assignments"),
        "the phantom assignments fetch must be gone. Got:\n{job_repo}"
    );

    let worker_repo =
        std::fs::read_to_string(output_dir.join("src/domain/core/worker/repository_impl.rs"))
            .unwrap();
    assert!(
        !worker_repo.contains("Column::Crew"),
        "the phantom crew fetch must be gone. Got:\n{worker_repo}"
    );

    let deployment_repo =
        std::fs::read_to_string(output_dir.join("src/domain/core/deployment/repository_impl.rs"))
            .unwrap();
    assert!(
        !deployment_repo.contains("Column::Deployment"),
        "no fetch may filter core_job/core_worker by a deployment-named column \
         (the decoys are not back-FKs). Got:\n{deployment_repo}"
    );
    assert!(
        !deployment_repo.contains("fn fetch_job_for_deployment")
            && !deployment_repo.contains("fn fetch_worker_for_deployment"),
        "array cross-references without a genuine back-FK must not emit fetches. \
         Got:\n{deployment_repo}"
    );

    let result = tokio::time::timeout(
        Duration::from_secs(300),
        tokio::process::Command::new("cargo")
            .args(["check"])
            .current_dir(output_dir)
            .output(),
    )
    .await;

    let output = match result {
        Ok(Ok(output)) => output,
        Ok(Err(e)) => panic!("failed to spawn cargo check: {e}"),
        Err(_) => {
            let preserved = tmp.keep();
            panic!(
                "cargo check timed out after 5 minutes!\n\
                 Temp dir preserved at: {}",
                preserved.display()
            );
        }
    };

    if !output.status.success() {
        let stdout = String::from_utf8_lossy(&output.stdout);
        let stderr = String::from_utf8_lossy(&output.stderr);
        let preserved = tmp.keep();
        panic!(
            "cargo check failed!\n\
             Temp dir preserved at: {}\n\
             --- stdout ---\n{}\n\
             --- stderr ---\n{}",
            preserved.display(),
            stdout,
            stderr
        );
    }
}

#[tokio::test]
async fn include_shapes_generated_code_compiles() {
    check_include_shapes_app().await;
}
