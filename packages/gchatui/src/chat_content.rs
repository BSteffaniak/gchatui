//! Google card/attachment conversion. Wire JSON never escapes this adapter.
use serde_json::Value;

use crate::model::RichContent;

pub fn convert(cards: &[Value], legacy: &[Value], attachments: &[Value]) -> Vec<RichContent> {
    let mut result = Vec::new();
    for card in cards.iter().map(|v| &v["card"]).chain(legacy.iter()) {
        let mut content = RichContent {
            title: text(&card["header"], "title").unwrap_or("Card").to_owned(),
            text: String::new(),
            image_url: None,
            links: Vec::new(),
        };
        if let Some(subtitle) = text(&card["header"], "subtitle") {
            line(&mut content.text, subtitle);
        }
        if let Some(sections) = card["sections"].as_array() {
            for section in sections {
                if let Some(header) = text(section, "header") {
                    line(&mut content.text, header);
                }
                if let Some(widgets) = section["widgets"].as_array() {
                    for widget in widgets {
                        widget_content(widget, &mut content, &mut result);
                    }
                }
            }
        }
        result.push(content);
    }
    for attachment in attachments {
        let mut content = RichContent {
            title: text(attachment, "contentName")
                .unwrap_or("Attachment")
                .to_owned(),
            text: text(attachment, "contentType").unwrap_or("File").to_owned(),
            image_url: attachment_image(attachment),
            links: Vec::new(),
        };
        if let Some(url) = text(attachment, "downloadUri").filter(|url| safe_url(url)) {
            content.links.push(("Open attachment".into(), url.into()));
        }
        result.push(content);
    }
    result
}

fn attachment_image(attachment: &Value) -> Option<String> {
    if text(attachment, "contentType").is_some_and(|kind| kind.starts_with("image/"))
        && let Some(resource) = text(&attachment["attachmentDataRef"], "resourceName")
        && !resource.is_empty()
    {
        let mut url = url::Url::parse("https://chat.googleapis.com/v1/media/")
            .expect("static media endpoint");
        url.path_segments_mut().ok()?.pop_if_empty().push(resource);
        url.query_pairs_mut().append_pair("alt", "media");
        return Some(url.into());
    }
    if !text(attachment, "contentType").is_some_and(|kind| kind.starts_with("image/")) {
        return None;
    }
    text(attachment, "thumbnailUri")
        .or_else(|| {
            text(attachment, "contentType")
                .filter(|kind| kind.starts_with("image/"))
                .and_then(|_| text(attachment, "downloadUri"))
        })
        .map(str::to_owned)
}

fn text<'a>(value: &'a Value, key: &str) -> Option<&'a str> {
    value[key].as_str().filter(|text| !text.is_empty())
}

fn line(output: &mut String, text: &str) {
    output.push_str(text);
    output.push('\n');
}

fn widget_content(widget: &Value, content: &mut RichContent, images: &mut Vec<RichContent>) {
    if let Some(value) = text(&widget["textParagraph"], "text") {
        line(&mut content.text, value);
        for fragment in value.split("href=\"").skip(1) {
            if let Some((url, _)) = fragment.split_once('"')
                && safe_url(url)
            {
                content.links.push(("Open text link".into(), url.into()));
            }
        }
    } else if widget
        .get("decoratedText")
        .or_else(|| widget.get("keyValue"))
        .is_some()
    {
        let field = widget
            .get("decoratedText")
            .unwrap_or_else(|| &widget["keyValue"]);
        for key in ["topLabel", "text", "content", "bottomLabel"] {
            if let Some(value) = text(field, key) {
                line(&mut content.text, value);
            }
        }
    } else if let Some(url) = text(&widget["image"], "imageUrl") {
        images.push(RichContent {
            title: text(&widget["image"], "altText").unwrap_or("Image").into(),
            text: String::new(),
            image_url: Some(url.into()),
            links: Vec::new(),
        });
    } else if widget.get("buttonList").is_none() && widget.get("buttons").is_none() {
        line(&mut content.text, "[Unsupported card widget]");
    }
    if let Some(buttons) = widget["buttonList"]["buttons"]
        .as_array()
        .or_else(|| widget["buttons"].as_array())
    {
        for button in buttons {
            let button = button.get("textButton").unwrap_or(button);
            let label = text(button, "text").unwrap_or("Link");
            if let Some(url) =
                text(&button["onClick"]["openLink"], "url").filter(|url| safe_url(url))
            {
                content.links.push((label.into(), url.into()));
            } else {
                line(
                    &mut content.text,
                    &format!("{label} [interactive action unavailable: read-only]"),
                );
            }
        }
    }
}

pub fn safe_url(value: &str) -> bool {
    url::Url::parse(value).is_ok_and(|url| {
        matches!(url.scheme(), "https" | "http")
            && url.host_str().is_some()
            && url.username().is_empty()
            && url.password().is_none()
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn preserves_cards_images_links_and_attachment_metadata() {
        let cards = [
            serde_json::json!({"card": {"header": {"title": "Synthetic card"}, "sections": [{"widgets": [
                {"textParagraph": {"text": "<b>Hello</b>"}},
                {"image": {"imageUrl": "https://example.com/image.png", "altText": "Sample"}},
                {"buttonList": {"buttons": [{"text": "Visit", "onClick": {"openLink": {"url": "https://example.com"}}}]}},
                {"selectionInput": {}}
            ]}]}}),
        ];
        let converted = convert(
            &cards,
            &[],
            &[serde_json::json!({"contentName": "sample.pdf", "contentType": "application/pdf"})],
        );
        assert_eq!(converted.len(), 3);
        assert_eq!(converted[0].title, "Sample");
        assert!(converted[1].text.contains("Hello"));
        assert!(converted[1].text.contains("Unsupported card widget"));
        assert_eq!(converted[1].links.len(), 1);
        assert_eq!(converted[2].title, "sample.pdf");
    }

    #[test]
    fn uploaded_images_use_authenticated_media_not_browser_downloads() {
        let attachment = serde_json::json!({"contentType": "image/png", "thumbnailUri": "https://example.com/thumbnail", "attachmentDataRef": {"resourceName": "spaces/sample/attachments/image"}});
        assert_eq!(
            attachment_image(&attachment).as_deref(),
            Some(
                "https://chat.googleapis.com/v1/media/spaces%2Fsample%2Fattachments%2Fimage?alt=media"
            )
        );
        let invalid = serde_json::json!({"contentType": "image/png", "attachmentDataRef": {"resourceName": "spaces/../attachments/image"}});
        assert!(
            attachment_image(&invalid)
                .unwrap()
                .contains("spaces%2F..%2Fattachments")
        );
    }

    #[test]
    fn opaque_media_references_are_encoded_without_assuming_resource_structure() {
        let attachment = serde_json::json!({"contentType": "image/png", "attachmentDataRef": {"resourceName": "synthetic+opaque=value"}});
        let url = attachment_image(&attachment).unwrap();
        assert!(url.starts_with("https://chat.googleapis.com/v1/media/"));
        assert!(url.ends_with("?alt=media"));
        let non_image = serde_json::json!({"contentType": "application/pdf", "thumbnailUri": "https://example.com/browser"});
        assert!(attachment_image(&non_image).is_none());
    }

    #[test]
    fn rejects_executable_and_credentialed_links() {
        for url in [
            "javascript:alert(1)",
            "file:///tmp/sample",
            "https://user:secret@example.com",
        ] {
            assert!(!safe_url(url));
        }
    }
}
