#![deny(clippy::unwrap_used, clippy::expect_used)]

mod application;
mod error;
mod owner;
mod project;
mod sandbox;

use clap::Parser;
use futures::StreamExt;
use std::sync::Arc;

#[derive(Parser)]
#[command(name = "vanyline-controller")]
struct Cli {
    /// Print CRD manifests and exit
    #[arg(long)]
    crds: bool,

    /// Tag de l'image sandbox (ghcr.io/sebt3/vanyline-sandbox) utilisée pour
    /// les pods sandbox et les Jobs de maintenance des projets, quand
    /// `Sandbox.spec.image` est absent. Défaut : version du controller — les
    /// composants sont versionnés et publiés ensemble.
    #[arg(long, env = "SANDBOX_IMAGE_TAG", default_value = env!("CARGO_PKG_VERSION"))]
    sandbox_image_tag: String,

    #[arg(
        long,
        env = "SANDBOX_IMAGE_REPO",
        default_value = "ghcr.io/sebt3/vanyline-sandbox"
    )]
    sandbox_image_repo: String,

    /// Tag de l'image app (ghcr.io/sebt3/vanyline-app) utilisée pour le
    /// Deployment `app`, quand `Application.spec.image` est absent.
    #[arg(long, env = "APP_IMAGE_TAG", default_value = env!("CARGO_PKG_VERSION"))]
    app_image_tag: String,

    #[arg(
        long,
        env = "APP_IMAGE_REPO",
        default_value = "ghcr.io/sebt3/vanyline-app"
    )]
    app_image_repo: String,
}

#[tokio::main]
async fn main() {
    let cli = Cli::parse();

    if cli.crds {
        print!("{}", vanyline_crds::crd_manifests());
        return;
    }

    let _ = tracing_subscriber::fmt::try_init();
    tracing::info!(
        "controller starting (owner + project + sandbox + application reconcilers active)"
    );

    let client = kube::Client::try_default().await.unwrap_or_else(|e| {
        tracing::error!(
            "failed to build kube client from in-cluster or kubeconfig context: {}",
            e
        );
        std::process::exit(1);
    });

    let sandbox_image = format!("{}:{}", cli.sandbox_image_repo, cli.sandbox_image_tag);
    let app_image = format!("{}:{}", cli.app_image_repo, cli.app_image_tag);

    let owner_ctx = Arc::new(owner::Context {
        client: client.clone(),
    });
    let project_ctx = Arc::new(project::Context {
        client: client.clone(),
        sandbox_image: sandbox_image.clone(),
    });

    let owner_run = owner::build_controller(client.clone())
        .run(owner::reconcile, owner::error_policy, owner_ctx)
        .for_each(|res| async move {
            if let Err(e) = res {
                tracing::warn!(error = %e, "owner reconcile loop error");
            }
        });

    let project_run = project::build_controller(client.clone())
        .run(project::reconcile, project::error_policy, project_ctx)
        .for_each(|res| async move {
            if let Err(e) = res {
                tracing::warn!(error = %e, "project reconcile loop error");
            }
        });

    let app_ctx = Arc::new(application::Context {
        client: client.clone(),
        default_image: app_image,
    });

    let application_run = application::build_controller(client.clone())
        .run(application::reconcile, application::error_policy, app_ctx)
        .for_each(|res| async move {
            if let Err(e) = res {
                tracing::warn!(error = %e, "application reconcile loop error");
            }
        });

    let sandbox_ctx = Arc::new(sandbox::Context {
        client: client.clone(),
        default_image: sandbox_image.clone(),
    });

    let sandbox_run = sandbox::build_controller(client)
        .run(sandbox::reconcile, sandbox::error_policy, sandbox_ctx)
        .for_each(|res| async move {
            if let Err(e) = res {
                tracing::warn!(error = %e, "sandbox reconcile loop error");
            }
        });

    tokio::join!(owner_run, project_run, sandbox_run, application_run);
}
