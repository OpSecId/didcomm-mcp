//! [User Profile 1.0](https://didcomm.org/user-profile/1.0): this agent's own profile
//! (display name, picture, description), the profiles peers send, and the protocol's
//! two messages for DIDComm v2 and v1.

use serde::{Deserialize, Serialize};
use serde_json::{json, Map, Value};

use didcomm_agent::DidcommVersion;

pub const USER_PROFILE: &str = "https://didcomm.org/user-profile/1.0";
pub const PROFILE: &str = "https://didcomm.org/user-profile/1.0/profile";
pub const REQUEST_PROFILE: &str = "https://didcomm.org/user-profile/1.0/request-profile";

/// Where profiles live in the store.
pub const OWN_KEY: &str = "profile";
pub const PEERS_KEY: &str = "peer_profiles";

/// The attachment id this agent's picture is sent as.
const PICTURE_ATTACHMENT: &str = "display-picture";
/// An embedded (base64) picture larger than this is dropped rather than stored.
const MAX_EMBEDDED_PICTURE: usize = 512 * 1024;

/// A profile: this agent's, or what a peer sent (`updated` is when it last changed).
#[derive(Debug, Clone, Default, Serialize, Deserialize, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct Profile {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub display_name: Option<String>,
    /// A URL (`https:`, or a `data:` URL for an embedded picture a peer sent).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub display_picture: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub description: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub updated: Option<i64>,
}

impl Profile {
    /// Empty strings are no value.
    pub fn normalized(self) -> Self {
        let clean = |v: Option<String>| v.map(|s| s.trim().to_string()).filter(|s| !s.is_empty());
        Self {
            display_name: clean(self.display_name),
            display_picture: clean(self.display_picture),
            description: clean(self.description),
            updated: self.updated,
        }
    }

    pub fn is_empty(&self) -> bool {
        self.display_name.is_none() && self.display_picture.is_none() && self.description.is_none()
    }
}

/// A `profile` message's body (v2) or fields (v1), with the picture as an attachment
/// linking to its URL. Only the `query`'d fields if given. Absent fields are sent as
/// `null`, which the protocol defines as "removed".
pub fn profile_message(profile: &Profile, query: Option<&[String]>, send_back_yours: bool, version: DidcommVersion) -> (Value, Option<Value>) {
    let wanted = |field: &str| query.is_none_or(|q| q.iter().any(|f| f == field));
    let mut fields = Map::new();
    if wanted("displayName") {
        fields.insert("displayName".into(), json!(profile.display_name));
    }
    if wanted("description") {
        fields.insert("description".into(), json!(profile.description));
    }
    let mut attachments = None;
    if wanted("displayPicture") {
        match &profile.display_picture {
            Some(url) => {
                fields.insert("displayPicture".into(), json!(format!("#{PICTURE_ATTACHMENT}")));
                let mut attachment = json!({"data": {"links": [url]}});
                match version {
                    DidcommVersion::V2 => attachment["id"] = json!(PICTURE_ATTACHMENT),
                    DidcommVersion::V1 => attachment["@id"] = json!(PICTURE_ATTACHMENT),
                }
                if let Some(media_type) = media_type_of(url) {
                    let key = if version == DidcommVersion::V2 { "media_type" } else { "mime-type" };
                    attachment[key] = json!(media_type);
                }
                attachments = Some(json!([attachment]));
            }
            None => {
                fields.insert("displayPicture".into(), Value::Null);
            }
        }
    }
    let mut body = json!({"profile": fields});
    if send_back_yours {
        body["send_back_yours"] = json!(true);
    }
    (body, attachments)
}

/// Apply a received `profile` message (v2 `body`/`attachments`, or v1 fields/`~attach`)
/// to what was known: absent fields keep their value, `null` or `""` remove it.
pub fn apply_profile(known: Option<Profile>, message: &Value, version: DidcommVersion, now: i64) -> Profile {
    let (fields, attachments) = match version {
        DidcommVersion::V2 => (&message["body"]["profile"], &message["attachments"]),
        DidcommVersion::V1 => (&message["profile"], &message["~attach"]),
    };
    let mut profile = known.unwrap_or_default();
    let text = |v: &Value| v.as_str().map(str::trim).filter(|s| !s.is_empty()).map(str::to_string);
    if let Some(v) = fields.get("displayName") {
        profile.display_name = text(v);
    }
    if let Some(v) = fields.get("description") {
        profile.description = text(v);
    }
    if let Some(v) = fields.get("displayPicture") {
        profile.display_picture = text(v).and_then(|reference| picture_url(&reference, attachments));
    }
    profile.updated = Some(now);
    profile
}

/// A `displayPicture` value as a URL: `#id` names an attachment (its first `https`
/// link, or its base64 data as a `data:` URL); a bare `https` URL is taken as is.
fn picture_url(reference: &str, attachments: &Value) -> Option<String> {
    let Some(id) = reference.strip_prefix('#') else {
        return safe_link(reference);
    };
    let attachment = attachments.as_array()?.iter().find(|a| a["id"] == id || a["@id"] == id)?;
    let data = &attachment["data"];
    if let Some(link) = data["links"].as_array().and_then(|l| l.iter().filter_map(Value::as_str).find_map(safe_link)) {
        return Some(link);
    }
    let base64 = data["base64"].as_str()?;
    if base64.len() > MAX_EMBEDDED_PICTURE * 4 / 3 {
        return None;
    }
    let media_type = attachment["media_type"]
        .as_str()
        .or(attachment["mime-type"].as_str())
        .filter(|m| m.starts_with("image/") && m.bytes().all(|b| b.is_ascii_alphanumeric() || b"/+.-".contains(&b)))
        .unwrap_or("image/png");
    let clean: String = base64.chars().filter(|c| c.is_ascii_alphanumeric() || "+/=".contains(*c)).collect();
    Some(format!("data:{media_type};base64,{clean}"))
}

/// Only `https:` (and `http:`) links are shown: never `javascript:` and the like.
fn safe_link(link: &str) -> Option<String> {
    let lower = link.trim().to_ascii_lowercase();
    (lower.starts_with("https://") || lower.starts_with("http://")).then(|| link.trim().to_string())
}

fn media_type_of(url: &str) -> Option<&'static str> {
    let path = url.split(['?', '#']).next()?.to_ascii_lowercase();
    Some(match path.rsplit('.').next()? {
        "png" => "image/png",
        "jpg" | "jpeg" => "image/jpeg",
        "gif" => "image/gif",
        "webp" => "image/webp",
        "svg" => "image/svg+xml",
        _ => return None,
    })
}

/// The `query` of a `request-profile` message, if it has one.
pub fn requested_fields(message: &Value, version: DidcommVersion) -> Option<Vec<String>> {
    let query = match version {
        DidcommVersion::V2 => &message["body"]["query"],
        DidcommVersion::V1 => &message["query"],
    };
    query.as_array().map(|q| q.iter().filter_map(|f| f.as_str().map(str::to_string)).collect())
}

/// Whether a `profile` message asks for ours back.
pub fn wants_ours_back(message: &Value, version: DidcommVersion) -> bool {
    match version {
        DidcommVersion::V2 => message["body"]["send_back_yours"] == true,
        DidcommVersion::V1 => message["send_back_yours"] == true,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn mine() -> Profile {
        Profile {
            display_name: Some("Main agent".into()),
            display_picture: Some("https://didcomm.link/a.png".into()),
            description: Some("bio".into()),
            updated: None,
        }
    }

    #[test]
    fn round_trips_through_its_own_messages() {
        for version in [DidcommVersion::V2, DidcommVersion::V1] {
            let (body, attachments) = profile_message(&mine(), None, true, version);
            let message = match version {
                DidcommVersion::V2 => json!({"body": body, "attachments": attachments}),
                DidcommVersion::V1 => {
                    let mut m = body.clone();
                    m["~attach"] = attachments.clone().unwrap();
                    m
                }
            };
            assert!(wants_ours_back(&message, version));
            let applied = apply_profile(None, &message, version, 7);
            assert_eq!(applied, Profile { updated: Some(7), ..mine() }, "{version:?}");
        }
    }

    #[test]
    fn absent_keeps_null_removes() {
        let known = mine();
        let update = json!({"body": {"profile": {"description": null, "displayName": "New"}}});
        let applied = apply_profile(Some(known), &update, DidcommVersion::V2, 1);
        assert_eq!(applied.display_name.as_deref(), Some("New"));
        assert_eq!(applied.description, None);
        assert_eq!(applied.display_picture.as_deref(), Some("https://didcomm.link/a.png"), "absent: unchanged");
    }

    #[test]
    fn pictures_embedded_or_unsafe() {
        let embedded = json!({"body": {"profile": {"displayPicture": "#p"}}, "attachments": [{"id": "p", "media_type": "image/jpeg", "data": {"base64": "aGk="}}]});
        assert_eq!(apply_profile(None, &embedded, DidcommVersion::V2, 1).display_picture.as_deref(), Some("data:image/jpeg;base64,aGk="));
        let evil = json!({"body": {"profile": {"displayPicture": "#p"}}, "attachments": [{"id": "p", "data": {"links": ["javascript:alert(1)"]}}]});
        assert_eq!(apply_profile(None, &evil, DidcommVersion::V2, 1).display_picture, None);
        let bad_type = json!({"body": {"profile": {"displayPicture": "#p"}}, "attachments": [{"id": "p", "media_type": "text/html", "data": {"base64": "aGk="}}]});
        assert_eq!(apply_profile(None, &bad_type, DidcommVersion::V2, 1).display_picture.as_deref(), Some("data:image/png;base64,aGk="));
    }

    #[test]
    fn query_limits_the_fields() {
        let (body, attachments) = profile_message(&mine(), Some(&["displayName".into()]), false, DidcommVersion::V2);
        assert_eq!(body, json!({"profile": {"displayName": "Main agent"}}));
        assert_eq!(attachments, None);
        let (body, _) = profile_message(&Profile::default(), None, false, DidcommVersion::V2);
        assert_eq!(body["profile"]["displayPicture"], Value::Null, "no picture: sent as removed");
    }
}
