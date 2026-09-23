//! Explicit read-only, bounded media diagnosis. Never emits API payloads or IDs.
use std::sync::Arc;

use anyhow::{Context, Result, bail};

pub async fn run() -> Result<()> {
    let config = crate::config::load(crate::config::default_config_path().as_deref())
        .map_err(|_| anyhow::anyhow!("cannot load application configuration"))?;
    if config.session_only || config.vault_passphrase {
        bail!(
            "media diagnosis requires an existing unlocked convenience-mode vault; no credentials changed"
        );
    }
    let client = crate::oauth::resolve_client(config.oauth_client_path.as_deref())
        .map_err(|_| anyhow::anyhow!("cannot resolve OAuth client"))?;
    let state = crate::config::default_state_dir().context("state directory unavailable")?;
    let (vault, identity) = crate::credential::client_auth_state_paths(&state, &client.client_id);
    if !vault.exists() || !identity.exists() {
        bail!("existing application credentials not found");
    }
    let store = Arc::new(crate::credential::SshenvCredentialStore::new(
        vault, identity,
    ));
    let auth = Arc::new(crate::auth::AuthManager::new(client, store));
    let token = auth.access_token().await.map_err(|_| {
        anyhow::anyhow!("vault authorization unavailable; sign in through the application")
    })?;
    let http = crate::google_http::GoogleHttp::new(Some(Arc::clone(&auth)));
    let spaces = http
        .get(
            "https://chat.googleapis.com/v1/spaces?pageSize=100".parse()?,
            &token,
        )
        .await
        .map_err(|_| anyhow::anyhow!("space request failed"))?;
    if !spaces.status().is_success() {
        bail!("space request HTTP {}", spaces.status().as_u16());
    }
    let spaces: serde_json::Value = spaces
        .json()
        .await
        .map_err(|_| anyhow::anyhow!("invalid spaces response"))?;
    let mut found = 0;
    for space in spaces["spaces"].as_array().into_iter().flatten().take(30) {
        let Some(name) = space["name"].as_str() else {
            continue;
        };
        let mut url = reqwest::Url::parse("https://chat.googleapis.com/v1/")?;
        url.path_segments_mut()
            .map_err(|()| anyhow::anyhow!("invalid API base"))?
            .pop_if_empty()
            .extend(name.split('/'))
            .push("messages");
        url.query_pairs_mut()
            .append_pair("pageSize", "100")
            .append_pair("orderBy", "createTime desc");
        let response = http
            .get(url, &token)
            .await
            .map_err(|_| anyhow::anyhow!("message request failed"))?;
        if !response.status().is_success() {
            continue;
        }
        let messages: serde_json::Value = response
            .json()
            .await
            .map_err(|_| anyhow::anyhow!("invalid messages response"))?;
        if inspect_messages(&messages, &auth, &mut found).await? {
            return Ok(());
        }
    }
    println!("diagnosis_complete attachments={found}");
    Ok(())
}

async fn inspect_messages(
    messages: &serde_json::Value,
    auth: &Arc<crate::auth::AuthManager>,
    found: &mut usize,
) -> Result<bool> {
    for message in messages["messages"].as_array().into_iter().flatten() {
        let cards = message["cardsV2"].as_array().map_or(&[][..], Vec::as_slice);
        let legacy = message["cards"].as_array().map_or(&[][..], Vec::as_slice);
        for content in crate::chat_content::convert(cards, legacy, &[]) {
            if let Some(url) = content.image_url {
                let parsed = reqwest::Url::parse(&url)?;
                let host = match parsed.host_str().unwrap_or("") {
                    "chat.google.com" => "chat",
                    "drive.google.com" => "drive",
                    "docs.google.com" => "docs",
                    "lh3.googleusercontent.com" => "googleusercontent",
                    _ => "other",
                };
                println!(
                    "card_image host_category={host} path_segments={} query_present={}",
                    parsed.path_segments().map_or(0, Iterator::count),
                    parsed.query().is_some()
                );
                let results = crate::rich_content::load(vec![url], Some(Arc::clone(auth))).await;
                for result in results.values() {
                    match result {
                        Ok(_) => println!("preview=ready"),
                        Err(error) => println!("preview={}", error.details()),
                    }
                }
                *found += 1;
                if *found >= 4 {
                    return Ok(true);
                }
            }
        }
        for attachment in message["attachment"].as_array().into_iter().flatten() {
            *found += 1;
            let resource = attachment["attachmentDataRef"]["resourceName"].as_str();
            let parts = resource
                .map(|value| value.split('/').collect::<Vec<_>>())
                .unwrap_or_default();
            println!(
                "attachment={found} image_type={} resource_present={} segments={} starts_spaces={} has_attachments={} thumbnail={} drive_ref={}",
                attachment["contentType"]
                    .as_str()
                    .is_some_and(|value| value.starts_with("image/")),
                resource.is_some(),
                parts.len(),
                parts.first() == Some(&"spaces"),
                parts.contains(&"attachments"),
                attachment["thumbnailUri"].is_string(),
                attachment.get("driveDataRef").is_some()
            );
            let content = crate::chat_content::convert(&[], &[], std::slice::from_ref(attachment));
            if let Some(url) = content
                .first()
                .and_then(|content| content.image_url.clone())
            {
                let results = crate::rich_content::load(vec![url], Some(Arc::clone(auth))).await;
                for result in results.values() {
                    match result {
                        Ok(_) => println!("preview=ready"),
                        Err(error) => println!("preview={}", error.details()),
                    }
                }
            }
            if *found >= 4 {
                return Ok(true);
            }
        }
    }
    Ok(false)
}
