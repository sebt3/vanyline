use vanyline_crds::{Application, Owner, OwnerSpec, Project, ProjectSpec, Sandbox, SandboxSpec};

use futures::{Stream, StreamExt};
use kube_runtime::watcher::{self, Config as WatcherConfig, Event};

use crate::error::VnyError;

/// Event de watch kube-runtime pour un Sandbox — normalisé pour la diffusion WS.
#[derive(Clone, Debug)]
pub enum WatchEvent<T> {
    Added(T),
    Modified(T),
    Deleted(T),
    Error(String),
}

/// Client K8s typé pour les CRDs Owner/Project/Sandbox — namespace résolu
/// par l'appelant (CLI, tâche 3 : `--namespace` > `defaults.namespace` du
/// config.yaml > namespace du contexte kubeconfig courant, cf. `discover`).
#[derive(Clone)]
pub struct VnlK8sClient {
    client: kube::Client,
    namespace: String,
}

impl VnlK8sClient {
    /// Découvre la config K8s (in-cluster, ou `~/.kube/config` sinon —
    /// `kube::Config::infer()` gère les deux). `namespace_override` prime
    /// sur le namespace du contexte kubeconfig courant si fourni (`Some`) ;
    /// sinon `Config::infer()` fournit déjà le default namespace résolu du
    /// contexte courant. Erreur `VNL-K8S-001` si aucune config n'est
    /// joignable (pas de cluster, pas de kubeconfig).
    pub async fn discover(namespace_override: Option<String>) -> Result<Self, VnyError> {
        let config = kube::Config::infer()
            .await
            .map_err(|e| VnyError::K8sConfigError(e.to_string()))?;
        let namespace = namespace_override.unwrap_or_else(|| config.default_namespace.clone());
        let client =
            kube::Client::try_from(config).map_err(|e| VnyError::K8sConfigError(e.to_string()))?;
        Ok(Self { client, namespace })
    }

    pub async fn list_owners(&self) -> Result<Vec<Owner>, VnyError> {
        list(&self.client, &self.namespace).await
    }
    pub async fn get_owner(&self, name: &str) -> Result<Owner, VnyError> {
        get(&self.client, &self.namespace, name).await
    }
    pub async fn create_owner(&self, name: &str, spec: OwnerSpec) -> Result<Owner, VnyError> {
        create(&self.client, &self.namespace, Owner::new(name, spec)).await
    }
    pub async fn delete_owner(&self, name: &str) -> Result<(), VnyError> {
        delete::<Owner>(&self.client, &self.namespace, name).await
    }

    pub async fn get_application(&self, name: &str) -> Result<Application, VnyError> {
        get(&self.client, &self.namespace, name).await
    }

    pub async fn list_projects(&self) -> Result<Vec<Project>, VnyError> {
        list(&self.client, &self.namespace).await
    }
    pub async fn get_project(&self, name: &str) -> Result<Project, VnyError> {
        get(&self.client, &self.namespace, name).await
    }
    pub async fn create_project(&self, name: &str, spec: ProjectSpec) -> Result<Project, VnyError> {
        create(&self.client, &self.namespace, Project::new(name, spec)).await
    }
    pub async fn delete_project(&self, name: &str) -> Result<(), VnyError> {
        delete::<Project>(&self.client, &self.namespace, name).await
    }

    pub async fn list_sandboxes(&self) -> Result<Vec<Sandbox>, VnyError> {
        list(&self.client, &self.namespace).await
    }
    pub async fn get_sandbox(&self, name: &str) -> Result<Sandbox, VnyError> {
        get(&self.client, &self.namespace, name).await
    }
    pub async fn create_sandbox(&self, name: &str, spec: SandboxSpec) -> Result<Sandbox, VnyError> {
        create(&self.client, &self.namespace, Sandbox::new(name, spec)).await
    }
    pub async fn delete_sandbox(&self, name: &str) -> Result<(), VnyError> {
        delete::<Sandbox>(&self.client, &self.namespace, name).await
    }

    /// Patch `spec.suspended` d'un Sandbox existant (merge patch JSON, pas de
    /// remplacement complet de la spec). Retourne l'objet patché (contrairement
    /// à `delete_sandbox` qui retourne `()`) — stop/start est une transition
    /// d'état, l'appelant (CLI/RPC) veut voir le nouveau statut immédiatement.
    pub async fn set_sandbox_suspended(
        &self,
        name: &str,
        suspended: bool,
    ) -> Result<Sandbox, VnyError> {
        let api: kube::Api<Sandbox> = kube::Api::namespaced(self.client.clone(), &self.namespace);
        let patch = serde_json::json!({ "spec": { "suspended": suspended } });
        api.patch(
            name,
            &kube::api::PatchParams::default(),
            &kube::api::Patch::Merge(&patch),
        )
        .await
        .map_err(|e| VnyError::K8sApiError(e.to_string()))
    }

    /// URL MCP HTTP-streamable de la sandbox `name`, posée par le
    /// controller (`vanyline_crds::service_name`/`MCP_PORT`). Vérifie
    /// d'abord que la sandbox existe (`get_sandbox`) — erreur claire si ce
    /// n'est pas le cas, plutôt qu'un échec de connexion confus plus tard
    /// dans le tour d'inférence (tâche 5).
    pub async fn sandbox_mcp_url(&self, name: &str) -> Result<String, VnyError> {
        self.get_sandbox(name).await?;
        Ok(format!(
            "http://{}.{}.svc:{}/mcp",
            vanyline_crds::service_name(name),
            self.namespace,
            vanyline_crds::MCP_PORT
        ))
    }

    /// URL interne de `POST /ws/ticket` de la sandbox `name` (même patron que
    /// `sandbox_mcp_url`, chemin `/ws/ticket`). Vérifie d'abord que la sandbox
    /// existe (`get_sandbox`) — erreur claire si ce n'est pas le cas, plutôt
    /// qu'un échec de connexion confus plus tard.
    pub async fn sandbox_ws_ticket_url(&self, name: &str) -> Result<String, VnyError> {
        self.get_sandbox(name).await?;
        Ok(format!(
            "http://{}.{}.svc:{}/ws/ticket",
            vanyline_crds::service_name(name),
            self.namespace,
            vanyline_crds::MCP_PORT
        ))
    }

    /// URL interne de `/git/*` de la sandbox `name` (même patron que
    /// `sandbox_ws_ticket_url`, chemin `/git/<raw_path>`). Vérifie d'abord que
    /// la sandbox existe (`get_sandbox`) — erreur claire si ce n'est pas le
    /// cas, plutôt qu'un échec de connexion confus plus tard.
    ///
    /// `raw_path` doit être **déjà** un chemin percent-encodé valide et
    /// validé par l'appelant (pas de segment `.`/`..`) — cette fonction ne
    /// ré-encode plus rien (contrairement à une version précédente qui
    /// appelait `encode_git_path` ici : décoder puis ré-encoder perdait la
    /// distinction entre un `%2F` légitime à l'intérieur d'un segment, ex.
    /// un nom de branche contenant `/`, et un vrai séparateur de chemin).
    /// Voir `app::api::sandboxes::raw_git_tail` pour la construction et la
    /// validation de `raw_path` à partir de la requête brute.
    pub async fn sandbox_git_url(&self, name: &str, raw_path: &str) -> Result<String, VnyError> {
        self.get_sandbox(name).await?;
        Ok(format!(
            "http://{}.{}.svc:{}/git/{}",
            vanyline_crds::service_name(name),
            self.namespace,
            vanyline_crds::MCP_PORT,
            raw_path
        ))
    }

    /// Retourne un stream de watch sur les CRD Sandbox du namespace. Le stream
    /// émet `WatchEvent::Added`/`Modified`/`Deleted` sur chaque objet et
    /// `WatchEvent::Error` sur toute erreur de connexion kube (l'appelant doit
    /// relancer la boucle externe en cas d'erreur).
    ///
    /// Boxé (`Pin<Box<dyn Stream>>`) plutôt que `impl Stream` : le stream est
    /// stocké derrière un champ dans le hub WS de `app`, un type nommé y est
    /// plus commode qu'un `impl Trait` opaque propagé partout.
    pub fn watch_sandboxes(
        &self,
    ) -> std::pin::Pin<Box<dyn Stream<Item = WatchEvent<Sandbox>> + Send>> {
        let api: kube::Api<Sandbox> = kube::Api::namespaced(self.client.clone(), &self.namespace);
        // timeout serveur 5 min : un reconnect trop court re-liste toutes les
        // sandboxes à chaque tour (bookmark perdu) → salve de refetch côté WS.
        let cfg = WatcherConfig::default().timeout(300);
        let stream = watcher::watcher(api, cfg).filter_map(|result| async move {
            match result {
                Ok(Event::Apply(obj)) => Some(WatchEvent::Modified(obj)),
                Ok(Event::Delete(obj)) => Some(WatchEvent::Deleted(obj)),
                Ok(Event::InitApply(obj)) => Some(WatchEvent::Added(obj)),
                // Init/InitDone : pas d'objet — ignorés (le replay initial se fait
                // via les InitApply ci-dessus, un par sandbox existante).
                Ok(Event::Init | Event::InitDone) => None,
                Err(e) => Some(WatchEvent::Error(e.to_string())),
            }
        });
        Box::pin(stream)
    }
}

async fn list<K>(client: &kube::Client, ns: &str) -> Result<Vec<K>, VnyError>
where
    K: kube::Resource<DynamicType = (), Scope = k8s_openapi::NamespaceResourceScope>
        + Clone
        + serde::de::DeserializeOwned
        + std::fmt::Debug,
{
    let api: kube::Api<K> = kube::Api::namespaced(client.clone(), ns);
    let list = api
        .list(&kube::api::ListParams::default())
        .await
        .map_err(|e| VnyError::K8sApiError(e.to_string()))?;
    Ok(list.items)
}

async fn get<K>(client: &kube::Client, ns: &str, name: &str) -> Result<K, VnyError>
where
    K: kube::Resource<DynamicType = (), Scope = k8s_openapi::NamespaceResourceScope>
        + Clone
        + serde::de::DeserializeOwned
        + std::fmt::Debug,
{
    let api: kube::Api<K> = kube::Api::namespaced(client.clone(), ns);
    api.get(name)
        .await
        .map_err(|e| VnyError::K8sApiError(e.to_string()))
}

async fn create<K>(client: &kube::Client, ns: &str, obj: K) -> Result<K, VnyError>
where
    K: kube::Resource<DynamicType = (), Scope = k8s_openapi::NamespaceResourceScope>
        + Clone
        + serde::de::DeserializeOwned
        + serde::Serialize
        + std::fmt::Debug,
{
    let api: kube::Api<K> = kube::Api::namespaced(client.clone(), ns);
    api.create(&kube::api::PostParams::default(), &obj)
        .await
        .map_err(|e| VnyError::K8sApiError(e.to_string()))
}

async fn delete<K>(client: &kube::Client, ns: &str, name: &str) -> Result<(), VnyError>
where
    K: kube::Resource<DynamicType = (), Scope = k8s_openapi::NamespaceResourceScope>
        + Clone
        + serde::de::DeserializeOwned
        + std::fmt::Debug,
{
    let api: kube::Api<K> = kube::Api::namespaced(client.clone(), ns);
    api.delete(name, &kube::api::DeleteParams::default())
        .await
        .map_err(|e| VnyError::K8sApiError(e.to_string()))?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use crate::encode_git_path;

    #[test]
    fn encode_git_path_preserves_separators() {
        assert_eq!(encode_git_path("branches/feature-x"), "branches/feature-x");
        assert_eq!(encode_git_path("status"), "status");
    }

    #[test]
    fn encode_git_path_encodes_special() {
        assert_eq!(
            encode_git_path("branches/feature x"),
            "branches/feature%20x"
        );
        // Le '%' est codé en %25, donc %20 devient %2520
        assert_eq!(encode_git_path("a%20b"), "a%2520b");
    }
}
