//! Résumé de compaction demandé au fournisseur d'inférence de la session.
//! Contrat : `summarizer.sdd`.

use std::sync::Arc;

use lymaille::{
    ContextError, ContextResult, Digest, ErrorClass, InferenceProvider, InferenceRequest, Message,
    ResolvedProfile, Summarizer, summary_prompt,
};

/// Résumeur qui demande le résumé au même fournisseur et avec le même profil que la session (`summarizer.sdd`,
/// `@ProviderSummarizer`).
pub struct ProviderSummarizer<P: InferenceProvider> {
    provider: Arc<P>,
    profile: ResolvedProfile,
}

impl<P: InferenceProvider> ProviderSummarizer<P> {
    /// Résumeur de la session : `provider` est le fournisseur de la session, `profile` son profil résolu.
    pub fn new(provider: Arc<P>, profile: ResolvedProfile) -> Self {
        Self { provider, profile }
    }
}

impl<P: InferenceProvider> Summarizer for ProviderSummarizer<P> {
    async fn summarize(&self, transcript: &str, digest: Option<&Digest>) -> ContextResult<String> {
        // Le digest, quand il est donné, précède le transcript dans le message utilisateur (`summarizer.sdd`, Handles).
        let contenu = match digest {
            Some(digest) => format!("{}\n\n{transcript}", digest.render()),
            None => transcript.to_owned(),
        };
        let requete = InferenceRequest::new(
            self.profile.clone(),
            summary_prompt(None).to_owned(),
            vec![Message::user(contenu)],
            Vec::new(),
            vec![None],
        );
        let reponse = self.provider.complete(requete).await.map_err(|erreur| {
            ContextError::Summarizer(format!(
                "{}: {}",
                classe_nom(erreur.class()),
                erreur.message()
            ))
        })?;
        let resume = match reponse.message() {
            Message::Assistant { content, .. } => content.clone(),
            _ => String::new(),
        };
        if resume.trim().is_empty() {
            return Err(ContextError::Summarizer("résumé vide".to_owned()));
        }
        Ok(resume)
    }
}

/// Nom d'une classe d'erreur d'inférence, tel que le message de résumé le nomme.
const fn classe_nom(classe: ErrorClass) -> &'static str {
    match classe {
        ErrorClass::Transient => "transient",
        ErrorClass::Permanent => "permanent",
        ErrorClass::ContextOverflow => "context_overflow",
        // `ErrorClass` est `#[non_exhaustive]` : une classe future se nomme sans être devinée.
        _ => "inconnue",
    }
}

#[cfg(test)]
mod tests {
    use std::sync::Arc;

    use lymaille::{
        ContextError, DEFAULT_SUMMARY_PROMPT, Digest, ErrorClass, InferenceError,
        InferenceResponse, Message, ModelProfile, ResolvedProfile, ScriptedProvider,
        Summarizer as _, summary_prompt,
    };

    use super::ProviderSummarizer;

    /// Profil de la session du test : celui qui est demandé au fournisseur.
    fn profil() -> ResolvedProfile {
        ModelProfile::new(
            "strix-qwen".to_owned(),
            "vllm-strix".to_owned(),
            "qwen3.8-flash-next".to_owned(),
        )
        .with_context_size(32_768)
        .resolve(None)
        .expect("un profil avec une taille de contexte se résout")
    }

    /// Fournisseur simulé qui répond `texte`, comme un message de l'assistant.
    fn repond(texte: &str) -> ScriptedProvider {
        ScriptedProvider::new(vec![Ok(InferenceResponse::new(
            Message::assistant(texte.to_owned(), None, Vec::new()),
            None,
        ))])
    }

    // Scenario: résumé rendu tel quel
    #[tokio::test]
    async fn resume_rendu_tel_quel() {
        let fournisseur = Arc::new(repond("résumé"));
        let resume = ProviderSummarizer::new(Arc::clone(&fournisseur), profil())
            .summarize("user: a\nassistant: b", None)
            .await;

        assert_eq!(
            resume.ok().as_deref(),
            Some("résumé"),
            "le texte de la réponse est rendu tel quel"
        );
        let requetes = fournisseur.requests();
        let requete = requetes
            .first()
            .expect("une requête de résumé doit être envoyée au fournisseur");
        assert_eq!(
            requete.system_prompt(),
            summary_prompt(None),
            "le message système est le prompt de résumé de lymaille"
        );
        assert_eq!(
            requete.system_prompt(),
            DEFAULT_SUMMARY_PROMPT,
            "sans surcharge, le prompt est celui de lymaille"
        );
        assert_eq!(
            requete.messages(),
            &[Message::user("user: a\nassistant: b")],
            "le message utilisateur est le transcript, sans autre message"
        );
        assert!(
            requete.tools().is_empty(),
            "la requête de résumé n'a aucun outil"
        );
        assert_eq!(
            requete.profile(),
            &profil(),
            "la requête porte le profil de la session"
        );
    }

    // Scenario: digest précédent
    #[tokio::test]
    async fn digest_precede_le_transcript() {
        let fournisseur = Arc::new(repond("résumé"));
        let digest = Digest::new(2, 5);
        let transcript = "user: a\nassistant: b";
        let resume = ProviderSummarizer::new(Arc::clone(&fournisseur), profil())
            .summarize(transcript, Some(&digest))
            .await;

        assert!(
            resume.is_ok(),
            "la compaction avec digest réussit, obtenu {resume:?}"
        );
        let requetes = fournisseur.requests();
        let requete = requetes
            .first()
            .expect("une requête de résumé doit être envoyée au fournisseur");
        let contenu = match requete.messages().first() {
            Some(Message::User { content }) => content.as_str(),
            autre => panic!("un message utilisateur est attendu, obtenu {autre:?}"),
        };
        let position_digest = contenu.find(&digest.render());
        let position_transcript = contenu.find(transcript);
        assert!(
            position_digest.is_some() && position_transcript.is_some(),
            "le message utilisateur porte le digest et le transcript, obtenu {contenu:?}"
        );
        assert!(
            position_digest < position_transcript,
            "le rendu du digest précède le transcript, obtenu {contenu:?}"
        );
    }

    // Scenario: réponse vide
    #[tokio::test]
    async fn reponse_blanche_est_refusee() {
        let fournisseur = Arc::new(repond("   \n\t "));
        let resume = ProviderSummarizer::new(fournisseur, profil())
            .summarize("user: a", None)
            .await;

        assert!(
            matches!(resume, Err(ContextError::Summarizer(_))),
            "une réponse blanche doit être refusée par ContextError::Summarizer, obtenu {resume:?}"
        );
    }

    // Scenario: échec d'inférence
    #[tokio::test]
    async fn echec_d_inference_nomme_la_classe_sans_corps() {
        let fournisseur = Arc::new(ScriptedProvider::new(vec![Err(InferenceError::new(
            ErrorClass::Transient,
            "service indisponible".to_owned(),
        ))]));
        let resume = ProviderSummarizer::new(fournisseur, profil())
            .summarize("user: texte confidentiel", None)
            .await;

        match resume {
            Err(ContextError::Summarizer(message)) => {
                assert!(
                    message.contains("transient"),
                    "le message nomme la classe de l'erreur, obtenu {message:?}"
                );
                assert!(
                    message.contains("service indisponible"),
                    "le message porte le message de l'erreur d'inférence, obtenu {message:?}"
                );
                assert!(
                    !message.contains("texte confidentiel"),
                    "le message ne porte aucun contenu de la requête, obtenu {message:?}"
                );
            }
            autre => panic!("une erreur ContextError::Summarizer est attendue, obtenu {autre:?}"),
        }
    }
}
