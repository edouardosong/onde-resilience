/// Reputation / Web-of-Trust — remplacement du PoW CPU fixe (Audit #11)
///
/// Principe :
/// - Chaque nœud possède un score de confiance `f64` (0.0 ..= 1.0).
/// - Les nœuds fondateurs (`genesis`) démarrent avec un score élevé.
/// - Un nœud de confiance (score > `ENDORSEMENT_THRESHOLD`) peut signer
///   (`endorse`) la clé d'un autre nœud ; l'approuvé gagne de la confiance.
/// - Les nœuds de confiance peuvent poster avec un PoW nul ou faible ;
///   les nœuds inconnus doivent "payer" un PoW adaptatif plus élevé.
///
/// Cette approche supprime le coût CPU constant (~65k SHA-256/msg à la
/// difficulté 4) : à 1M messages/jour, le réseau ne s'effondre plus sous
/// sa propre charge anti-spam — le coût est concentré sur les nœuds qui
/// n'ont pas encore prouvé leur fiabilité.
use std::collections::HashMap;
use serde::{Deserialize, Serialize};

/// Seuil au-dessus duquel un nœud est considéré "de confiance"
/// (peut poster sans PoW, peut endosser d'autres nœuds).
pub const TRUSTED_THRESHOLD: f64 = 0.7;
/// Seuil minimum pour qu'une signature d'endossement soit acceptée.
pub const ENDORSEMENT_THRESHOLD: f64 = 0.7;
/// Décroissance de confiance transmise par un endossement (0.8 × 0.5 = 0.4).
pub const ENDORSEMENT_DECAY: f64 = 0.5;
/// Score initial des nœuds fondateurs.
pub const GENESIS_TRUST: f64 = 0.8;
/// Score d'un nœud totalement inconnu.
pub const UNKNOWN_TRUST: f64 = 0.0;
/// Nombre minimal d'endossements par des nœuds > TRUSTED_THRESHOLD
/// pour devenir "de confiance".
pub const REQUIRED_ENDORSEMENTS: usize = 3;
/// Difficulté PoW maximale imposée à un nœud sans réputation.
pub const MAX_POW_DIFFICULTY: u8 = 4;
/// Difficulté PoW minimale (réseau) pour les nœuds inconnus.
pub const BASE_POW_DIFFICULTY: u8 = 2;

/// Un endossement : le nœud `endorser` signe la clé publique du nœud
/// `endorsed` à l'instant `timestamp`.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct Endorsement {
    pub endorser: String,
    pub endorsed: String,
    pub timestamp: u64,
}

/// Système de réputation décentralisé (Web of Trust).
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct ReputationSystem {
    /// pubkey_hex → score de confiance (0.0 ..= 1.0)
    scores: HashMap<String, f64>,
    /// pubkey_hex → endossements reçus (par d'autres nœuds)
    endorsements: HashMap<String, Vec<Endorsement>>,
    /// pubkey_hex → nombre de signatures vérifiées de ce nœud
    activity: HashMap<String, u64>,
}

impl ReputationSystem {
    pub fn new() -> Self {
        Self::default()
    }

    /// Initialiser le réseau avec des nœuds fondateurs de confiance.
    pub fn bootstrap(&mut self, genesis_peers: &[String]) {
        for peer in genesis_peers {
            self.set_trusted(peer, GENESIS_TRUST);
        }
    }

    /// Fixer directement le score d'un nœud (admin / fondateur).
    pub fn set_trusted(&mut self, pubkey: &str, score: f64) {
        let score = score.clamp(0.0, 1.0);
        self.scores.insert(pubkey.to_string(), score);
    }

    /// Score actuel d'un nœud (inconnu → `UNKNOWN_TRUST`).
    pub fn score(&self, pubkey: &str) -> f64 {
        self.scores.get(pubkey).copied().unwrap_or(UNKNOWN_TRUST)
    }

    /// Un nœud est-il considéré de confiance ?
    pub fn is_trusted(&self, pubkey: &str) -> bool {
        self.score(pubkey) >= TRUSTED_THRESHOLD
    }

    /// Enregistrer un endossement d'un nœud déjà de confiance.
    ///
    /// L'endosseur doit avoir un score >= `ENDORSEMENT_THRESHOLD`.
    /// Le score de l'endossé est mis à jour :
    ///   new = max(current, endorser_score × ENDORSEMENT_DECAY)
    /// Après `REQUIRED_ENDORSEMENTS` endossements qualifiés, le nœud
    /// devient "de confiance" (score >= TRUSTED_THRESHOLD).
    pub fn endorse(&mut self, endorser: &str, endorsed: &str, timestamp: u64) -> Result<f64, String> {
        if endorser == endorsed {
            return Err("A node cannot endorse itself".to_string());
        }
        if self.score(endorser) < ENDORSEMENT_THRESHOLD {
            return Err(format!(
                "Endorser {endorser} is not trusted enough (score {})",
                self.score(endorser)
            ));
        }

        let list = self.endorsements.entry(endorsed.to_string()).or_default();
        if list.iter().any(|e| e.endorser == endorser) {
            return Err("Duplicate endorsement".to_string());
        }
        list.push(Endorsement {
            endorser: endorser.to_string(),
            endorsed: endorsed.to_string(),
            timestamp,
        });

        let endorser_score = self.score(endorser);
        let gained = endorser_score * ENDORSEMENT_DECAY;
        let current = self.score(endorsed);
        let new_score = current.max(gained);
        self.scores.insert(endorsed.to_string(), new_score);

        // Promotion automatique après suffisamment d'endossements qualifiés
        // (calculé sans garder l'emprunt mutable sur `endorsements`)
        let qualified = self.endorsements
            .get(endorsed)
            .map(|l| {
                l.iter()
                    .filter(|e| self.scores.get(&e.endorser).copied().unwrap_or(0.0) >= ENDORSEMENT_THRESHOLD)
                    .count()
            })
            .unwrap_or(0);
        if qualified >= REQUIRED_ENDORSEMENTS && new_score < TRUSTED_THRESHOLD {
            self.scores.insert(endorsed.to_string(), TRUSTED_THRESHOLD);
        }

        Ok(self.score(endorsed))
    }

    /// Appliquer un endossement **reçu d'un autre nœud** (propagation WoT,
    /// Phase 1.2).
    ///
    /// Réutilise intégralement la logique qualifiée de [`Self::endorse`]
    /// (anti-self, anti-doublon, seuil de l'endosseur, promotion) sans la
    /// dupliquer. La seule différence : la signature de l'endosseur a déjà été
    /// vérifiée par l'appelant (couche gossip / `Node`) — `endorser_sig_verified`
    /// documente ce fait et un endossement non vérifié est rejeté d'office.
    pub fn apply_remote_endorsement(
        &mut self,
        endorsement: &Endorsement,
        endorser_sig_verified: bool,
    ) -> Result<(), String> {
        if !endorser_sig_verified {
            return Err("Endorsement signature could not be verified".to_string());
        }
        self.endorse(&endorsement.endorser, &endorsement.endorsed, endorsement.timestamp)
            .map(|_| ())
    }

    /// Endossements actuellement intégrés par ce nœud — jeu candidat pour le
    /// **relai** : un nœud qui a intégré un endossement peut le rediffuser
    /// dans le gossip (chaque receveur le re-publie vers les pairs qui ne
    /// l'ont pas encore reçu, la déduplication restant assurée par le gossip).
    pub fn pending_endorsements(&self) -> Vec<Endorsement> {
        self.endorsements.values().flatten().cloned().collect()
    }

    /// Compter les endossements qualifiés reçus par un nœud.
    pub fn endorsement_count(&self, pubkey: &str) -> usize {
        self.endorsements
            .get(pubkey)
            .map(|l| {
                l.iter()
                    .filter(|e| self.scores.get(&e.endorser).copied().unwrap_or(0.0) >= ENDORSEMENT_THRESHOLD)
                    .count()
            })
            .unwrap_or(0)
    }

    /// Enregistrer une activité positive (message valide reçu) d'un nœud.
    pub fn record_activity(&mut self, pubkey: &str) {
        let n = self.activity.entry(pubkey.to_string()).or_insert(0);
        *n += 1;
        // Petit bonus de réputation par activité vérifiée (plafonné)
        if *n >= 10 {
            let s = self.score(pubkey);
            if s < TRUSTED_THRESHOLD {
                let boosted = (s + 0.02).min(TRUSTED_THRESHOLD);
                self.scores.insert(pubkey.to_string(), boosted);
            }
        }
    }

    /// Signaler un comportement abusif (spam, signature invalide).
    pub fn penalize(&mut self, pubkey: &str, amount: f64) {
        let s = self.score(pubkey);
        self.scores.insert(pubkey.to_string(), (s - amount).max(0.0));
    }

    /// Difficulté PoW adaptative requise pour un expéditeur.
    ///
    /// - Nœud de confiance (score >= TRUSTED_THRESHOLD) → 0 (pas de PoW)
    /// - Nœud inconnu → `BASE_POW_DIFFICULTY` (plancher réseau)
    /// - Nœud intermédiaire → échelle linéaire entre 0 et MAX
    ///   proportionnelle à l'inverse de la réputation.
    pub fn required_pow_difficulty(&self, pubkey: &str) -> u8 {
        let score = self.score(pubkey);
        if score >= TRUSTED_THRESHOLD {
            return 0;
        }
        if score <= UNKNOWN_TRUST {
            return MAX_POW_DIFFICULTY;
        }
        // 0 < score < 0.7 : difficulté entre BASE et MAX, inversement
        // proportionnelle au score. Ex. score 0.35 → ~3.
        let t = score / TRUSTED_THRESHOLD; // 0..1
        let diff_f = MAX_POW_DIFFICULTY as f64 - t * (MAX_POW_DIFFICULTY - BASE_POW_DIFFICULTY) as f64;
        (diff_f.round() as u8).clamp(BASE_POW_DIFFICULTY, MAX_POW_DIFFICULTY)
    }

    /// Nombre de nœuds suivis.
    pub fn known_count(&self) -> usize {
        self.scores.len()
    }

    /// Résumé du système (pour l'UI / le débogage).
    pub fn summary(&self) -> Vec<(String, f64, usize)> {
        let mut v: Vec<(String, f64, usize)> = self
            .scores
            .iter()
            .map(|(k, s)| (k.clone(), *s, self.endorsement_count(k)))
            .collect();
        v.sort_by(|a, b| b.1.partial_cmp(&a.1).unwrap_or(std::cmp::Ordering::Equal));
        v
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn key(n: &str) -> String {
        format!("{n}{}", "a".repeat(64 - n.len()))
    }

    #[test]
    fn test_unknown_node_max_pow() {
        let rep = ReputationSystem::new();
        assert_eq!(rep.required_pow_difficulty(&key("u")), MAX_POW_DIFFICULTY);
        assert!(!rep.is_trusted(&key("u")));
    }

    #[test]
    fn test_genesis_trusted_no_pow() {
        let mut rep = ReputationSystem::new();
        rep.bootstrap(&[key("g1"), key("g2")]);
        assert!(rep.is_trusted(&key("g1")));
        assert_eq!(rep.required_pow_difficulty(&key("g1")), 0);
    }

    #[test]
    fn test_endorsement_chain() {
        let mut rep = ReputationSystem::new();
        let g1 = key("g1");
        let g2 = key("g2");
        let g3 = key("g3");
        rep.bootstrap(&[g1.clone(), g2.clone(), g3.clone()]);

        let newcomer = key("n");
        // Avant tout endossement : inconnu, PoW max
        assert_eq!(rep.required_pow_difficulty(&newcomer), MAX_POW_DIFFICULTY);

        // Un seul endossement d'un nœud de confiance (0.8 × 0.5 = 0.4)
        rep.endorse(&g1, &newcomer, 1_000).unwrap();
        assert_eq!(rep.score(&newcomer), 0.4);
        assert!(!rep.is_trusted(&newcomer));
        // Difficulté adaptative intermédiaire
        let d = rep.required_pow_difficulty(&newcomer);
        assert!((BASE_POW_DIFFICULTY..MAX_POW_DIFFICULTY).contains(&d));

        // 3 endossements de nœuds de confiance → promotion
        rep.endorse(&g2, &newcomer, 1_001).unwrap();
        rep.endorse(&g3, &newcomer, 1_002).unwrap();
        assert!(rep.is_trusted(&newcomer), "3 endorsements must promote");
        assert_eq!(rep.required_pow_difficulty(&newcomer), 0);
        assert_eq!(rep.endorsement_count(&newcomer), 3);
    }

    #[test]
    fn test_self_endorsement_rejected() {
        let mut rep = ReputationSystem::new();
        let g = key("g");
        rep.bootstrap(std::slice::from_ref(&g));
        assert!(rep.endorse(&g, &g, 1_000).is_err());
    }

    #[test]
    fn test_untrusted_endorser_rejected() {
        let mut rep = ReputationSystem::new();
        let unknown = key("u");
        let target = key("t");
        assert!(
            rep.endorse(&unknown, &target, 1_000).is_err(),
            "unknown endorser must be rejected"
        );
    }

    #[test]
    fn test_duplicate_endorsement_rejected() {
        let mut rep = ReputationSystem::new();
        let g = key("g");
        rep.bootstrap(std::slice::from_ref(&g));
        let t = key("t");
        rep.endorse(&g, &t, 1_000).unwrap();
        assert!(rep.endorse(&g, &t, 1_001).is_err());
    }

    #[test]
    fn test_activity_bonus_and_penalty() {
        let mut rep = ReputationSystem::new();
        let n = key("n");
        for _ in 0..10 {
            rep.record_activity(&n);
        }
        assert!(rep.score(&n) > 0.0, "activity must increase score");

        rep.penalize(&n, 0.5);
        assert_eq!(rep.score(&n), 0.0);
    }

    #[test]
    fn test_apply_remote_endorsement_reuses_endorse() {
        // Phase 1.2 : `apply_remote_endorsement` réutilise la logique `endorse`
        // (anti-self, anti-doublon, seuil de l'endosseur) — les mêmes règles
        // s'appliquent à un endossement reçu du réseau.
        let mut rep = ReputationSystem::new();
        let g1 = key("g1");
        let g2 = key("g2");
        let g3 = key("g3");
        rep.bootstrap(&[g1.clone(), g2.clone(), g3.clone()]);
        let newcomer = key("n");

        // Un endossement non vérifié (signature absente) est rejeté d'office.
        let unverified = Endorsement {
            endorser: g1.clone(),
            endorsed: newcomer.clone(),
            timestamp: 1_000,
        };
        assert!(
            rep.apply_remote_endorsement(&unverified, false).is_err(),
            "unverified endorsement must be rejected"
        );
        assert_eq!(rep.score(&newcomer), 0.0, "nothing applied");

        // Un endossement vérifié d'un endosseur de confiance s'applique (0.4).
        let verified = Endorsement {
            endorser: g1.clone(),
            endorsed: newcomer.clone(),
            timestamp: 1_001,
        };
        rep.apply_remote_endorsement(&verified, true).unwrap();
        assert_eq!(rep.score(&newcomer), 0.4);
        assert_eq!(rep.endorsement_count(&newcomer), 1);

        // Le doublon (même endosseur) est rejeté par la logique `endorse`.
        let dup = Endorsement {
            endorser: g1.clone(),
            endorsed: newcomer.clone(),
            timestamp: 1_002,
        };
        assert!(
            rep.apply_remote_endorsement(&dup, true).is_err(),
            "duplicate endorsement must be rejected"
        );
        assert_eq!(rep.endorsement_count(&newcomer), 1);

        // Un endossement d'un nœud NON de confiance est ignoré.
        let unknown = key("u");
        let from_unknown = Endorsement {
            endorser: unknown.clone(),
            endorsed: newcomer.clone(),
            timestamp: 1_003,
        };
        assert!(
            rep.apply_remote_endorsement(&from_unknown, true).is_err(),
            "unknown endorser must be rejected"
        );

        // Un auto-endossement reçu du réseau est rejeté.
        let self_end = Endorsement {
            endorser: g1.clone(),
            endorsed: g1.clone(),
            timestamp: 1_004,
        };
        assert!(rep.apply_remote_endorsement(&self_end, true).is_err());

        // Promotion : 3 endossements qualifiés de nœuds de confiance rendent
        // le nouveau venu "de confiance" — exactement comme `endorse`.
        rep.apply_remote_endorsement(
            &Endorsement { endorser: g2.clone(), endorsed: newcomer.clone(), timestamp: 1_005 },
            true,
        )
        .unwrap();
        rep.apply_remote_endorsement(
            &Endorsement { endorser: g3.clone(), endorsed: newcomer.clone(), timestamp: 1_006 },
            true,
        )
        .unwrap();
        assert!(rep.is_trusted(&newcomer), "3 qualified endorsements must promote");
        assert_eq!(rep.score(&newcomer), TRUSTED_THRESHOLD);
    }

    #[test]
    fn test_pending_endorsements_for_relay() {
        // Phase 1.2 : les endossements intégrés sont exposés pour le relai.
        let mut rep = ReputationSystem::new();
        let g1 = key("g1");
        let g2 = key("g2");
        rep.bootstrap(&[g1.clone(), g2.clone()]);
        let a = key("a");
        let b = key("b");

        rep.apply_remote_endorsement(
            &Endorsement { endorser: g1.clone(), endorsed: a.clone(), timestamp: 1 },
            true,
        )
        .unwrap();
        rep.apply_remote_endorsement(
            &Endorsement { endorser: g2.clone(), endorsed: b.clone(), timestamp: 2 },
            true,
        )
        .unwrap();

        let pending = rep.pending_endorsements();
        assert_eq!(pending.len(), 2, "integrated endorsements are relayable");
        assert!(pending.iter().any(|e| e.endorser == g1 && e.endorsed == a));
        assert!(pending.iter().any(|e| e.endorser == g2 && e.endorsed == b));

        // Un doublon rejeté n'ajoute rien au jeu de relai.
        let dup = Endorsement { endorser: g1.clone(), endorsed: a.clone(), timestamp: 3 };
        assert!(rep.apply_remote_endorsement(&dup, true).is_err());
        assert_eq!(rep.pending_endorsements().len(), 2);
    }
}
