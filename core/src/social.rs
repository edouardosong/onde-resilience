//! Contrat social partagé entre Tuitter et Redit.
//!
//! Les objets sociaux sont sérialisables dans des événements ONDE signés afin
//! de rester compatibles avec le fonctionnement hors ligne et le gossip.

use serde::{Deserialize, Serialize};

use crate::crypto::Identity;
use crate::protocol::{MeshEvent, OndeMessageType};

pub const MAX_TUITTER_BODY: usize = 500;
pub const MAX_REDIT_TITLE: usize = 300;
pub const MAX_REDIT_BODY: usize = 40_000;
pub const MAX_COMMUNITY_SLUG: usize = 50;
/// Corps d'un message privé Tuitter (bornage domaine — payload wire).
pub const MAX_PRIVATE_MESSAGE_BODY: usize = 2_000;
/// Motif d'un signalement de modération (bornage domaine — payload wire).
pub const MAX_MODERATION_REASON: usize = 500;
/// Longueur maximale d'un identifiant social (id de post/commentaire/cible).
pub const MAX_SOCIAL_ID: usize = 128;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum SocialPlatform {
    Tuitter,
    Redit,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct SocialPost {
    pub id: String,
    pub platform: SocialPlatform,
    pub author_pubkey: String,
    pub title: Option<String>,
    pub body: String,
    pub community_slug: Option<String>,
    /// Hérité du prototype Fusion puis RETIRÉ (I1) : l'imbrication appartient
    /// aux commentaires ([`SocialComment::parent_id`]). Le champ n'est plus
    /// sérialisé ; un payload wire historique portant encore `parent_id` est
    /// accepté (serde ignore les champs inconnus) et la valeur est ignorée.
    pub media_urls: Vec<String>,
}

impl SocialPost {
    pub fn validate(&self) -> Result<(), String> {
        if self.id.is_empty() || self.id.chars().count() > 128 {
            return Err("social post id must contain 1..=128 characters".to_string());
        }
        if self.author_pubkey.len() != 64 || hex::decode(&self.author_pubkey).is_err() {
            return Err("social post author_pubkey must be a 32-byte hex key".to_string());
        }
        if self.body.trim().is_empty() {
            return Err("social post body cannot be empty".to_string());
        }
        if self.media_urls.len() > 4 {
            return Err("social post cannot contain more than 4 media URLs".to_string());
        }

        match self.platform {
            SocialPlatform::Tuitter => {
                if self.title.is_some() {
                    return Err("Tuitter posts cannot have a title".to_string());
                }
                if self.body.chars().count() > MAX_TUITTER_BODY {
                    return Err(format!(
                        "Tuitter body exceeds {MAX_TUITTER_BODY} characters"
                    ));
                }
                if self.community_slug.is_some() {
                    return Err("Tuitter posts cannot target a Redit community".to_string());
                }
            }
            SocialPlatform::Redit => {
                let title = self
                    .title
                    .as_deref()
                    .ok_or_else(|| "Redit posts require a title".to_string())?;
                if title.trim().is_empty() || title.chars().count() > MAX_REDIT_TITLE {
                    return Err(format!(
                        "Redit title must contain 1..={MAX_REDIT_TITLE} characters"
                    ));
                }
                if self.body.chars().count() > MAX_REDIT_BODY {
                    return Err(format!("Redit body exceeds {MAX_REDIT_BODY} characters"));
                }
                let slug = self
                    .community_slug
                    .as_deref()
                    .ok_or_else(|| "Redit posts require a community".to_string())?;
                if slug.is_empty() || slug.chars().count() > MAX_COMMUNITY_SLUG {
                    return Err(format!(
                        "community slug must contain 1..={MAX_COMMUNITY_SLUG} characters"
                    ));
                }
                if !slug.chars().all(|character| {
                    character.is_ascii_lowercase() || character.is_ascii_digit() || character == '-'
                }) {
                    return Err("community slug contains invalid characters".to_string());
                }
            }
        }

        Ok(())
    }

    /// Encode the post as a signed ONDE event ready for gossip.
    pub fn to_mesh_event(&self, sender: &Identity) -> Result<MeshEvent, String> {
        self.validate()?;
        if self.author_pubkey != sender.pubkey_hex() {
            return Err("social post author does not match signing identity".to_string());
        }
        let kind = match self.platform {
            SocialPlatform::Tuitter => OndeMessageType::SocialPost,
            SocialPlatform::Redit => OndeMessageType::SocialPost,
        };
        let content = serde_json::to_string(self)
            .map_err(|error| format!("failed to encode social post: {error}"))?;
        let mut tags = vec![format!("platform={:?}", self.platform)];
        if let Some(slug) = &self.community_slug {
            tags.push(format!("community={slug}"));
        }
        let mut event = MeshEvent::new_signed(sender, kind, content, tags);
        if !event.compute_pow(1_000_000) {
            return Err("failed to compute proof of work for social post".to_string());
        }
        Ok(event)
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct SocialComment {
    pub id: String,
    pub platform: SocialPlatform,
    pub author_pubkey: String,
    pub post_id: String,
    pub parent_id: Option<String>,
    pub body: String,
}

impl SocialComment {
    pub fn validate(&self) -> Result<(), String> {
        if self.id.is_empty() || self.id.chars().count() > 128 {
            return Err("comment id must contain 1..=128 characters".to_string());
        }
        if self.post_id.is_empty() || self.post_id.chars().count() > 128 {
            return Err("comment post_id must contain 1..=128 characters".to_string());
        }
        if self.parent_id.as_deref() == Some(self.id.as_str()) {
            return Err("comment cannot be its own parent".to_string());
        }
        if self.body.trim().is_empty() || self.body.chars().count() > MAX_REDIT_BODY {
            return Err("comment body must contain 1..=40000 characters".to_string());
        }
        if self.author_pubkey.len() != 64 || hex::decode(&self.author_pubkey).is_err() {
            return Err("comment author_pubkey must be a 32-byte hex key".to_string());
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn post(platform: SocialPlatform, identity: &Identity) -> SocialPost {
        SocialPost {
            id: "post-1".to_string(),
            platform,
            author_pubkey: identity.pubkey_hex(),
            title: None,
            body: "message hors ligne".to_string(),
            community_slug: None,
            media_urls: Vec::new(),
        }
    }

    #[test]
    fn validates_tuitter_post_and_emits_signed_event() {
        let identity = Identity::generate();
        let social_post = post(SocialPlatform::Tuitter, &identity);

        let event = social_post.to_mesh_event(&identity).expect("valid post");

        assert_eq!(event.kind, OndeMessageType::SocialPost);
        assert_eq!(event.pubkey, identity.pubkey_hex());
        assert!(event.validate().is_ok());
        let decoded: SocialPost = serde_json::from_str(&event.content).expect("valid payload");
        assert_eq!(decoded, social_post);
    }

    #[test]
    fn rejects_cross_platform_post_rules() {
        let identity = Identity::generate();
        let mut tuitter = post(SocialPlatform::Tuitter, &identity);
        tuitter.title = Some("titre interdit".to_string());
        assert!(tuitter.validate().is_err());

        let mut redit = post(SocialPlatform::Redit, &identity);
        redit.title = Some("Discussion".to_string());
        assert!(redit.validate().is_err());

        redit.community_slug = Some("entraide".to_string());
        assert!(redit.validate().is_ok());
    }

    #[test]
    fn rejects_wrong_signing_identity() {
        let identity = Identity::generate();
        let other = Identity::generate();
        let social_post = post(SocialPlatform::Tuitter, &identity);

        let error = social_post
            .to_mesh_event(&other)
            .expect_err("identity mismatch");
        assert!(error.contains("signing identity"));
    }

    #[test]
    fn validates_nested_comments() {
        let identity = Identity::generate();
        let comment = SocialComment {
            id: "comment-1".to_string(),
            platform: SocialPlatform::Redit,
            author_pubkey: identity.pubkey_hex(),
            post_id: "post-1".to_string(),
            parent_id: Some("comment-0".to_string()),
            body: "réponse".to_string(),
        };
        assert!(comment.validate().is_ok());

        let mut invalid = comment;
        invalid.parent_id = Some(invalid.id.clone());
        assert!(invalid.validate().is_err());
    }
}
