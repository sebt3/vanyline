use vanyline_crds::{Application, Owner, OwnerSpec, Project, ProjectSpec, Sandbox, SandboxSpec};

use crate::error::VnyError;

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
