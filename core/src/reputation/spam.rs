//! Anti-abus — détection de spam et pénalités de réputation propagées
//! (ROADMAP Phase 2.7).
//!
//! Complète la WoT existante ([`crate::reputation::ReputationSystem`]) :
//! - **détection** : fenêtre glissante par auteur ([`SpamGuard`]) — plafonne le
//!   débit admissible bien sous les caps mémoire du gossip (`MAX_*`) ;
//! - **pénalités locales** : chaque violation **attribuable** (l'événement est
//!   signé — une signature invalide n'est PAS attribuable et n'est jamais
//!   pénalisée) augmente le niveau d'abus de l'auteur ;
//! - **actions graduées** : [`TrustAction`] — Accept / Throttle /
//!   Deprioritize / Ignore selon des seuils documentés ;
//! - **remontée lente** : l'abus décroît de [`ABUSE_RECOVERY_PER_HOUR`] par
//!   heure (granularité heure entière, arithmétique entière déterministe) ;
//! - **propagation** : [`AbuseReport`] signé, diffusé comme un endossement
//!   négatif (kind wire 15) — intégré seulement si le rapporteur est **de
//!   confiance** localement, dédupliqué par (rapporteur, coupable, raison).
//!
//! Déterminisme : toutes les fonctions prennent `now` (unix secs) en
//! paramètre explicite — aucun appel horloge système ici, les tests pilotent
//! le temps.

use serde::{Deserialize, Serialize};
use std::collections::{HashMap, VecDeque};

// ---------------------------------------------------------------------------
// Constantes (documentées — valeurs par défaut cohérentes entre pairs)
// ---------------------------------------------------------------------------

/// Fenêtre glissante anti-spam par auteur (secondes).
pub const SPAM_WINDOW_SECS: u64 = 60;
/// Nombre maximal d'événements **admis** par auteur dans une fenêtre.
///
/// Calibrage : un auteur honnête de confiance publie au plus 1 message /
/// 10 s ([`crate::node::PUBLISH_INTERVAL_TRUSTED_SECS`] ≈ 6/min) ; le budget
/// 12/fenêtre laisse un facteur 2 de marge tout en bornant un flood à ~20 %
/// d'une fenêtre de gossip (`MAX_PENDING_BROADCASTS = 1000`).
pub const SPAM_BUDGET_PER_WINDOW: usize = 12;
/// Nombre maximal d'auteurs suivis par le garde-fou (borne mémoire).
pub const MAX_TRACKED_AUTHORS: usize = 1024;
/// Nombre maximal de signalements d'abus distincts mémorisés (dédup).
pub const MAX_ABUSE_REPORTS_TRACKED: usize = 2048;

/// Niveau d'abus à partir duquel l'auteur est throttled (budget réduit,
/// PoW maximal exigé).
pub const ABUSE_THROTTLE_THRESHOLD: f64 = 0.15;
/// Niveau d'abus à partir duquel l'auteur est dépriorisé.
pub const ABUSE_DEPRIORITIZE_THRESHOLD: f64 = 0.40;
/// Niveau d'abus à partir duquel l'auteur est ignoré (messages jetés).
pub const ABUSE_IGNORE_THRESHOLD: f64 = 0.80;

/// Pénalité locale : débit excessif (violation de la fenêtre glissante).
pub const PENALTY_EXCESSIVE_RATE: f64 = 0.30;
/// Pénalité locale : événement signé mais invalide (payload, timestamp…).
pub const PENALTY_INVALID_EVENT: f64 = 0.10;
/// Pénalité locale : PoW insuffisant pour le plancher exigé.
pub const PENALTY_INSUFFICIENT_POW: f64 = 0.20;
/// Poids d'un signalement distant **qualifié** (rapporteur de confiance).
pub const PENALTY_REMOTE_REPORT: f64 = 0.10;

/// Remontée lente : décroissance du niveau d'abus par heure écoulée.
/// Effacer 0.80 (seuil d'ignorance) demande donc ~80 h de bon comportement.
pub const ABUSE_RECOVERY_PER_HOUR: f64 = 0.01;
/// Secondes par heure (arithmétique entière de la décroissance).
pub const SECS_PER_HOUR: u64 = 3600;

// ---------------------------------------------------------------------------
// Raisons d'abus attribuables (codes wire stables)
// ---------------------------------------------------------------------------

/// Raison d'une violation — uniquement des causes **attribuables** :
/// l'auteur a signé l'événement fautif. Une signature invalide n'est jamais
/// une raison (attribution impossible : n'importe qui peut forger).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[repr(u8)]
pub enum AbuseReason {
    /// Débit excessif (fenêtre glissante dépassée).
    ExcessiveRate = 1,
    /// Événement signé mais invalide (payload malformé, timestamp…).
    InvalidEvent = 2,
    /// PoW insuffisant pour le plancher exigé.
    InsufficientPow = 3,
}

impl AbuseReason {
    /// Code wire numérique (stable — ne jamais renuméroter).
    pub fn code(self) -> u8 {
        self as u8
    }

    /// Retrouver la raison depuis son code wire (`None` si inconnu — un
    /// signalement portant une raison inconnue est rejeté).
    pub fn from_code(code: u8) -> Option<Self> {
        match code {
            1 => Some(AbuseReason::ExcessiveRate),
            2 => Some(AbuseReason::InvalidEvent),
            3 => Some(AbuseReason::InsufficientPow),
            _ => None,
        }
    }

    /// Pénalité locale associée à cette raison.
    pub fn penalty(self) -> f64 {
        match self {
            AbuseReason::ExcessiveRate => PENALTY_EXCESSIVE_RATE,
            AbuseReason::InvalidEvent => PENALTY_INVALID_EVENT,
            AbuseReason::InsufficientPow => PENALTY_INSUFFICIENT_POW,
        }
    }
}

// ---------------------------------------------------------------------------
// Actions graduées
// ---------------------------------------------------------------------------

/// Action décidée localement vis-à-vis d'un auteur, selon son niveau d'abus.
///
/// Un auteur **sans aucun historique d'abus** est toujours `Accept` — quel que
/// soit son score de confiance WoT (même inconnu) : les pairs honnêtes ne
/// sont jamais affectés par le dispositif anti-abus.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TrustAction {
    /// Traitement normal (PoW adaptatif WoT existant).
    Accept,
    /// Budget de fenêtre réduit + PoW maximal exigé.
    Throttle,
    /// Dépriorisé : PoW maximal + traitement après les pairs sains.
    Deprioritize,
    /// Ignoré : messages jetés à l'entrée (contenu).
    Ignore,
}

/// Action pour un niveau d'abus donné (fonction pure — testable).
pub fn action_for_abuse(abuse: f64) -> TrustAction {
    if abuse >= ABUSE_IGNORE_THRESHOLD {
        TrustAction::Ignore
    } else if abuse >= ABUSE_DEPRIORITIZE_THRESHOLD {
        TrustAction::Deprioritize
    } else if abuse >= ABUSE_THROTTLE_THRESHOLD {
        TrustAction::Throttle
    } else {
        TrustAction::Accept
    }
}

/// Décroissance lente : niveau d'abus vu à `now` pour un enregistrement
/// `(score, updated_at)`. Granularité **heure entière** (division entière) —
/// déterministe et monotone.
pub fn decayed_abuse(score: f64, updated_at: u64, now: u64) -> f64 {
    let full_hours = now.saturating_sub(updated_at) / SECS_PER_HOUR;
    (score - full_hours as f64 * ABUSE_RECOVERY_PER_HOUR).max(0.0)
}

/// Enregistrement d'abus d'un auteur (score actif + date du dernier événement).
#[derive(Debug, Clone, Default, Serialize, Deserialize, PartialEq)]
pub struct AbuseRecord {
    /// Niveau d'abus actif au moment `updated_at` (0.0 ..= 1.0).
    pub score: f64,
    /// Date (unix secs) du dernier enregistrement/décroissance pliée.
    pub updated_at: u64,
}

impl AbuseRecord {
    /// Niveau d'abus vu à `now` (décroissance pliée à la lecture).
    pub fn level_at(&self, now: u64) -> f64 {
        decayed_abuse(self.score, self.updated_at, now)
    }

    /// Enregistrer une nouvelle violation d'amont `amount` à `now`.
    /// Retourne le nouveau niveau (saturé à 1.0).
    pub fn record(&mut self, amount: f64, now: u64) -> f64 {
        // Plier la décroissance AVANT d'ajouter : la pénalité repart toujours
        // du niveau réellement actif à l'instant de la nouvelle violation.
        let current = self.level_at(now);
        self.score = (current + amount).min(1.0);
        self.updated_at = now;
        self.score
    }
}

// ---------------------------------------------------------------------------
// Signalement d'abus propagé (endossement négatif)
// ---------------------------------------------------------------------------

/// Signalement d'abus — l'équivalent **négatif** de [`crate::reputation::Endorsement`].
///
/// Wire format : sérialisé JSON puis base64 dans le `content` d'un
/// `MeshEvent` de kind `AbuseReport` (code 15) ; `sig` = signature Ed25519 du
/// rapporteur sur l'ID canonique ; PoW adaptatif identique aux autres kinds.
/// Le receveur intègre le signalement **seulement si le rapporteur est de
/// confiance dans SA vue locale** (miroir exact de la règle des endossements)
/// — un inconnu ne peut ni endosser ni dénoncer.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct AbuseReport {
    /// Clé publique hex du rapporteur (= `pubkey` signé de l'événement).
    pub reporter: String,
    /// Clé publique hex de l'auteur fautif visé.
    pub offender: String,
    /// Code numérique de la [`AbuseReason`].
    pub reason: u8,
    /// Horodatage unix (secs) du constat par le rapporteur.
    pub timestamp: u64,
}

// ---------------------------------------------------------------------------
// Fenêtre glissante par auteur
// ---------------------------------------------------------------------------

/// Fenêtre glissante des admissions par auteur.
///
/// Mémoire bornée deux fois : `stamps.len() <= budget` par auteur (on
/// n'enregistre QUE les admissions) et nombre d'auteurs suivi borné par
/// [`MAX_TRACKED_AUTHORS`] (éviction du moins récemment actif).
#[derive(Debug, Clone)]
pub struct SpamGuard {
    window_secs: u64,
    budget: usize,
    entries: HashMap<String, AuthorWindow>,
}

#[derive(Debug, Clone, Default)]
struct AuthorWindow {
    /// Horodatages (unix secs) des admissions encore dans la fenêtre.
    stamps: VecDeque<u64>,
}

impl SpamGuard {
    pub fn new(window_secs: u64, budget: usize) -> Self {
        Self {
            window_secs,
            budget,
            entries: HashMap::new(),
        }
    }

    pub fn window_secs(&self) -> u64 {
        self.window_secs
    }

    pub fn budget(&self) -> usize {
        self.budget
    }

    /// Nombre d'auteurs actuellement suivis (visibilité/borne mémoire).
    pub fn tracked_authors(&self) -> usize {
        self.entries.len()
    }

    /// Admettre (ou non) un événement supplémentaire de `author` à `now`.
    ///
    /// `true` = autorisé ET enregistré dans la fenêtre ; `false` = refusé,
    /// **rien n'est enregistré** (le compteur ne monte que sur ce qui passe —
    /// l'attaque ne peut pas se auto-blinder).
    pub fn admit(&mut self, author: &str, now: u64) -> bool {
        if self.budget == 0 {
            return false;
        }
        let window = self.entries.entry(author.to_string()).or_default();
        // Purger les admissions sorties de la fenêtre (front = plus ancien).
        let horizon = now.saturating_sub(self.window_secs);
        while window.stamps.front().copied().is_some_and(|t| t < horizon) {
            window.stamps.pop_front();
        }
        if window.stamps.len() >= self.budget {
            // Refus : rien n'est enregistré (le refus ne consomme pas le budget).
            if window.stamps.is_empty() {
                self.entries.remove(author);
            }
            return false;
        }
        window.stamps.push_back(now);
        // Borne mémoire : éviction du moins récemment actif au-delà du cap.
        if self.entries.len() > MAX_TRACKED_AUTHORS {
            self.evict_oldest();
        }
        true
    }

    /// Évincer l'auteur le moins récemment actif (dernière admission la plus
    /// ancienne). O(n) sur un cap borné (`MAX_TRACKED_AUTHORS`) — appelé
    /// uniquement à l'entrée d'un NOUVEL auteur au-delà du cap.
    fn evict_oldest(&mut self) {
        let mut oldest_key: Option<String> = None;
        let mut oldest_time = u64::MAX;
        for (k, w) in &self.entries {
            let last = w.stamps.back().copied().unwrap_or(0);
            if last < oldest_time {
                oldest_time = last;
                oldest_key = Some(k.clone());
            }
        }
        if let Some(k) = oldest_key {
            self.entries.remove(&k);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn key(n: &str) -> String {
        format!("{n}{}", "a".repeat(64 - n.len()))
    }

    // ── Codes de raison ──

    #[test]
    fn test_abuse_reason_codes_stable() {
        assert_eq!(AbuseReason::ExcessiveRate.code(), 1);
        assert_eq!(AbuseReason::InvalidEvent.code(), 2);
        assert_eq!(AbuseReason::InsufficientPow.code(), 3);
        for r in [
            AbuseReason::ExcessiveRate,
            AbuseReason::InvalidEvent,
            AbuseReason::InsufficientPow,
        ] {
            assert_eq!(AbuseReason::from_code(r.code()), Some(r));
        }
        assert_eq!(AbuseReason::from_code(0), None);
        assert_eq!(AbuseReason::from_code(4), None);
        assert_eq!(AbuseReason::from_code(255), None);
        // Pénalités ordonnées : le débit excessif coûte plus cher qu'un
        // payload invalide isolé, le PoW manquant entre les deux.
        const {
            assert!(PENALTY_INVALID_EVENT < PENALTY_INSUFFICIENT_POW);
            assert!(PENALTY_INSUFFICIENT_POW < PENALTY_EXCESSIVE_RATE);
        }
    }

    // ── Seuils d'action ──

    #[test]
    fn test_action_thresholds_graduated() {
        assert_eq!(action_for_abuse(0.0), TrustAction::Accept);
        assert_eq!(
            action_for_abuse(ABUSE_THROTTLE_THRESHOLD),
            TrustAction::Throttle
        );
        assert_eq!(
            action_for_abuse(ABUSE_DEPRIORITIZE_THRESHOLD),
            TrustAction::Deprioritize
        );
        assert_eq!(
            action_for_abuse(ABUSE_IGNORE_THRESHOLD),
            TrustAction::Ignore
        );
        assert_eq!(action_for_abuse(1.0), TrustAction::Ignore);
        // Juste sous chaque seuil : palier inférieur
        assert_eq!(action_for_abuse(0.14), TrustAction::Accept);
        assert_eq!(action_for_abuse(0.39), TrustAction::Throttle);
        assert_eq!(action_for_abuse(0.79), TrustAction::Deprioritize);
    }

    // ── Décroissance lente ──

    #[test]
    fn test_decayed_abuse_hourly_recovery() {
        // Pas de temps écoulé → pas de récupération
        assert!((decayed_abuse(0.9, 1_000_000, 1_000_000) - 0.9).abs() < 1e-9);
        // 59 min → aucune heure pleine → rien ne bouge (granularité heure)
        assert!((decayed_abuse(0.9, 1_000_000, 1_000_000 + 3599) - 0.9).abs() < 1e-9);
        // 1 h exactement → -ABUSE_RECOVERY_PER_HOUR
        let got = decayed_abuse(0.9, 1_000_000, 1_000_000 + SECS_PER_HOUR);
        assert!((got - (0.9 - ABUSE_RECOVERY_PER_HOUR)).abs() < 1e-9);
        // 3.5 h → 3 heures pleines
        let got = decayed_abuse(0.9, 1_000_000, 1_000_000 + 3 * SECS_PER_HOUR + 1799);
        assert!((got - (0.9 - 3.0 * ABUSE_RECOVERY_PER_HOUR)).abs() < 1e-9);
        // Jamais sous zéro, même très longtemps après
        assert_eq!(
            decayed_abuse(0.5, 1_000_000, 1_000_000 + 10_000 * SECS_PER_HOUR),
            0.0
        );
        // Horloge en retard (now < updated_at) → saturating, pas de panique
        assert!((decayed_abuse(0.5, 1_000_000, 999_999) - 0.5).abs() < 1e-9);
    }

    #[test]
    fn test_abuse_record_accumulates_and_decays() {
        let mut rec = AbuseRecord::default();
        let t0 = 1_000_000u64;
        // Trois violations de débit rapprochées : 0.3 → 0.6 → 0.9
        assert!((rec.record(PENALTY_EXCESSIVE_RATE, t0) - 0.3).abs() < 1e-9);
        assert!((rec.record(PENALTY_EXCESSIVE_RATE, t0 + 1) - 0.6).abs() < 1e-9);
        assert!((rec.record(PENALTY_EXCESSIVE_RATE, t0 + 2) - 0.9).abs() < 1e-9);
        // Saturation à 1.0
        assert_eq!(rec.record(PENALTY_EXCESSIVE_RATE, t0 + 3), 1.0);
        assert_eq!(rec.record(PENALTY_EXCESSIVE_RATE, t0 + 4), 1.0);
        // Dernière écriture à t0+3 ; après exactement 10 h pleines :
        // 1.0 - 10 × ABUSE_RECOVERY_PER_HOUR = 0.90
        let t_read = t0 + 4 + 10 * SECS_PER_HOUR;
        let lvl = rec.level_at(t_read);
        assert!((lvl - 0.90).abs() < 1e-9);
        // …et la prochaine violation part du niveau DÉCAYÉ (pliée à l'écriture)
        let lvl = rec.record(PENALTY_EXCESSIVE_RATE, t_read);
        assert!(
            (lvl - 1.0).abs() < 1e-9,
            "0.90 + 0.30 saturates back at 1.0"
        );
    }

    // ── Fenêtre glissante ──

    #[test]
    fn test_spam_guard_budget_then_window_slide() {
        let mut guard = SpamGuard::new(SPAM_WINDOW_SECS, SPAM_BUDGET_PER_WINDOW);
        let a = key("attacker");
        let t0 = 50_000u64;
        // Budget exact admis
        for i in 0..SPAM_BUDGET_PER_WINDOW {
            assert!(guard.admit(&a, t0 + i as u64), "admit #{i} must pass");
        }
        assert_eq!(guard.tracked_authors(), 1);
        // Au-delà : refusé, et SANS consommer le budget
        for i in 0..5 {
            assert!(!guard.admit(&a, t0 + SPAM_BUDGET_PER_WINDOW as u64 + i as u64));
        }
        // Fenêtre glissante : à t0+window+ε, les vieilles admissions sortent
        assert!(guard.admit(&a, t0 + SPAM_WINDOW_SECS + 1));
        // Les stamps expirés ont été purgés : il n'y a que les nouvelles
        // admissions + les éventuelles dans [slide, slide] dans la fenêtre
        assert!(guard.admit(&a, t0 + SPAM_WINDOW_SECS + 2));
    }

    #[test]
    fn test_spam_guard_isolated_per_author() {
        let mut guard = SpamGuard::new(SPAM_WINDOW_SECS, 3);
        let a = key("alice");
        let b = key("bob");
        let t0 = 10_000u64;
        assert!(guard.admit(&a, t0));
        assert!(guard.admit(&a, t0 + 1));
        assert!(guard.admit(&a, t0 + 2));
        assert!(!guard.admit(&a, t0 + 3), "alice exhausted her own budget");
        // Bob indépendant : budget plein disponible
        assert!(guard.admit(&b, t0 + 3));
        assert!(guard.admit(&b, t0 + 4));
        assert!(guard.admit(&b, t0 + 5));
        assert!(!guard.admit(&b, t0 + 6));
        assert_eq!(guard.tracked_authors(), 2);
    }

    #[test]
    fn test_spam_guard_memory_bound_on_authors() {
        let mut guard = SpamGuard::new(SPAM_WINDOW_SECS, 2);
        let t0 = 7_000u64;
        // Plus d'auteurs que MAX_TRACKED_AUTHORS : la borne doit tenir
        for i in 0..(MAX_TRACKED_AUTHORS + 50) {
            guard.admit(&key(&format!("k{i}")), t0 + i as u64 % 5);
        }
        assert!(
            guard.tracked_authors() <= MAX_TRACKED_AUTHORS,
            "tracked authors {} must stay <= {MAX_TRACKED_AUTHORS}",
            guard.tracked_authors()
        );
        // Le garde reste fonctionnel après éviction
        assert!(guard.admit(&key("fresh"), t0 + 99));
    }

    #[test]
    fn test_spam_guard_zero_budget_never_admits() {
        // Configuration dégénérée : budget 0 → tout refusé, sans panique
        let mut guard = SpamGuard::new(SPAM_WINDOW_SECS, 0);
        assert!(!guard.admit(&key("x"), 1_000));
        assert_eq!(guard.tracked_authors(), 0);
    }
}
