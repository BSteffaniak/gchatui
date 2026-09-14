use std::sync::Arc;

use hyperchad::app::AppBuilder;
use hyperchad::color::Color;
use hyperchad::router::Router;
use hyperchad::template::{Containers, container};

fn home() -> Containers {
    container! {
        div padding=32 background="#0d1117" color="#c9d1d9" font-family="monospace" {
            div color="#7ee787" font-size=14 margin-bottom=24 { "BMUX / GCHATUI" }
            h1 color="#f0f6fc" font-size=42 margin-bottom=16 { "Google Chat. In your terminal." }
            div font-size=18 margin-bottom=32 { "Read conversations, messages, and thread replies with first-class mouse and keyboard navigation." }
            div background="#161b22" padding=24 border-radius=8 border="1, #30363d" margin-bottom=24 {
                h2 color="#7ee787" font-size=24 margin-bottom=12 { "Your account. Your device." }
                div margin-bottom=12 { "Direct access to Google. Local credential custody. No conversation-history cache or hosted token broker." }
                div { "Read-only first release for macOS, Linux, and Windows." }
            }
            h2 font-size=24 margin-bottom=12 { "Early development" }
            div margin-bottom=24 { "Official OAuth distribution and verification are in progress. Current builds require your own private desktop client configuration. Workspace policies still apply." }
            anchor href="https://github.com/BSteffaniak/gchatui" color="#79c0ff" margin-bottom=16 { "Source and setup instructions →" }
            anchor href="https://github.com/BSteffaniak/gchatui/releases" color="#79c0ff" margin-bottom=16 { "Releases →" }
            anchor href="/privacy" color="#79c0ff" margin-bottom=16 { "Privacy →" }
            anchor href="https://github.com/BSteffaniak/gchatui/issues" color="#79c0ff" { "Public support — never post credentials or private conversations →" }
        }
    }
}

fn privacy() -> Containers {
    container! {
        div padding=32 background="#0d1117" color="#c9d1d9" font-family="monospace" {
            anchor href="/" color="#79c0ff" margin-bottom=24 { "← gchatui" }
            h1 font-size=36 color="#f0f6fc" margin-bottom=24 { "Privacy information" }
            div color="#e3b341" margin-bottom=24 { "Pre-release notice: a finalized policy with operator, private contact, and effective date is required before Google verification." }
            h2 font-size=24 margin-bottom=12 { "Desktop application" }
            div margin-bottom=16 { "With consent, gchatui reads Chat spaces, messages, replies, and memberships. Profile, contacts, and directory access resolve names. The first release does not send or modify messages." }
            div margin-bottom=16 { "The app connects directly to Google. Conversations and name indexes stay in memory; no conversation history is persisted or uploaded to a gchatui server. Imported aliases stay local." }
            div margin-bottom=16 { "Access tokens stay in memory. Session-only mode persists no tokens. Persistent mode stores the refresh token in an app-specific encrypted sshenv vault. The identity is passphrase-protected by default. Disabling its passphrase explicitly relies on filesystem permissions; copying both identity and vault permits token recovery." }
            div margin-bottom=24 { "The current app has no analytics or conversation-upload service. Google data is not used for advertising or model training. Workspace administrators can observe or block OAuth access." }
            h2 font-size=24 margin-bottom=12 { "Your controls" }
            div margin-bottom=16 { "Decline consent, close the app, or revoke authorization through Google Account connections. Revocation does not delete local files; remove app-specific credentials and aliases separately. Do not remove a global sshenv vault. Backups may retain local files." }
            anchor href="https://myaccount.google.com/connections" color="#79c0ff" margin-bottom=24 { "Google Account connections →" }
            h2 font-size=24 margin-bottom=12 { "Website" }
            div margin-bottom=16 { "Cloudflare serves these pages and processes ordinary request information, including IP addresses and request metadata, to deliver and protect the site. No analytics scripts or forms are added. This website does not handle Google callbacks or tokens." }
            anchor href="https://www.cloudflare.com/privacypolicy/" color="#79c0ff" margin-bottom=16 { "Cloudflare privacy policy →" }
            anchor href="https://github.com/BSteffaniak/gchatui/issues" color="#79c0ff" { "Public support — non-sensitive questions only →" }
        }
    }
}

fn main() -> Result<(), Box<dyn std::error::Error>> {
    let runtime = Arc::new(switchy::unsync::runtime::Builder::new().build()?);
    let router = Router::new()
        .with_static_route("/", |_| async { home() })
        .with_static_route("/privacy", |_| async { privacy() });
    AppBuilder::new()
        .with_router(router)
        .with_background(Color::from_hex("#0d1117"))
        .with_title("gchatui — Google Chat in your terminal".to_string())
        .with_description("A read-only Google Chat terminal client".to_string())
        .with_viewport("width=device-width, initial-scale=1".to_string())
        .with_size(1100.0, 700.0)
        .with_runtime_handle(runtime.handle())
        .build_default()?
        .run()?;
    Ok(())
}
