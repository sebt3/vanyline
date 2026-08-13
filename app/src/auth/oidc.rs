use openidconnect::{
    core::{CoreAuthenticationFlow, CoreClient, CoreProviderMetadata},
    AuthorizationCode, ClientId, ClientSecret, CsrfToken, EndpointMaybeSet, EndpointNotSet,
    EndpointSet, HttpRequest, HttpResponse, IssuerUrl, Nonce, RedirectUrl, Scope, TokenResponse,
};

type BuiltCoreClient = CoreClient<
    EndpointSet,
    EndpointNotSet,
    EndpointNotSet,
    EndpointNotSet,
    EndpointMaybeSet,
    EndpointMaybeSet,
>;

use crate::config::Config;
use crate::error::AppError;

#[async_trait::async_trait]
pub trait OidcClientTrait: Send + Sync {
    fn authorization_url(&self) -> (openidconnect::url::Url, CsrfToken, Nonce);
    async fn exchange_code(
        &self,
        code: &str,
        expected_nonce: &Nonce,
    ) -> Result<(String, String), AppError>;
}

pub struct OidcClient {
    inner: BuiltCoreClient,
    http_client: reqwest::Client,
    scopes: Vec<String>,
}

fn build_http_client(config: &Config) -> Result<reqwest::Client, AppError> {
    let mut builder = reqwest::Client::builder().redirect(reqwest::redirect::Policy::none());

    if let Some(ref ca_pem) = config.oidc_ca_cert {
        let cert = reqwest::Certificate::from_pem(ca_pem.as_bytes())
            .map_err(|e| AppError::OidcError(format!("VNL-AUTH-007: invalid OIDC CA cert: {e}")))?;
        builder = builder.add_root_certificate(cert);
    }

    builder.build().map_err(|e| {
        AppError::OidcError(format!(
            "VNL-AUTH-007: failed to build OIDC HTTP client: {e}"
        ))
    })
}

#[allow(clippy::expect_used)] // signature imposee par openidconnect::discover_async (reqwest::Error) ; construction de reponse a partir d un status+headers deja valides, infaillible en pratique
async fn send_http_request(
    client: &reqwest::Client,
    request: HttpRequest,
) -> Result<HttpResponse, reqwest::Error> {
    let (parts, body) = request.into_parts();
    let mut builder = client.request(
        reqwest::Method::from_bytes(parts.method.as_str().as_bytes())
            .unwrap_or(reqwest::Method::GET),
        parts.uri.to_string(),
    );
    for (name, value) in &parts.headers {
        builder = builder.header(name, value);
    }
    let response = builder.body(body).send().await?;

    let status = response.status();
    let mut response_builder = openidconnect::http::Response::builder().status(status);
    if let Some(headers) = response_builder.headers_mut() {
        headers.extend(response.headers().clone());
    }
    let body = response.bytes().await?.to_vec();
    Ok(response_builder
        .body(body)
        .expect("VNL-AUTH-008: failed to build HTTP response"))
}

impl OidcClient {
    pub async fn new(config: &Config) -> Result<Self, AppError> {
        let issuer_url = IssuerUrl::new(config.oidc_issuer_url.clone()).map_err(|e| {
            AppError::OidcError(format!("VNL-AUTH-004: invalid OIDC issuer URL: {e:?}"))
        })?;

        let http_client = build_http_client(config)?;

        let provider_metadata = {
            let client = http_client.clone();
            CoreProviderMetadata::discover_async(issuer_url, &move |req: HttpRequest| {
                let client = client.clone();
                async move { send_http_request(&client, req).await }
            })
            .await
            .map_err(|e| {
                AppError::OidcError(format!("VNL-AUTH-005: OIDC discovery failed: {e:?}"))
            })?
        };

        let redirect_url = RedirectUrl::new(config.oidc_redirect_url.clone()).map_err(|e| {
            AppError::OidcError(format!("VNL-AUTH-006: invalid OIDC redirect URL: {e:?}"))
        })?;

        let client = CoreClient::from_provider_metadata(
            provider_metadata,
            ClientId::new(config.oidc_client_id.clone()),
            Some(ClientSecret::new(config.oidc_client_secret.clone())),
        )
        .set_redirect_uri(redirect_url);

        Ok(Self {
            inner: client,
            http_client,
            scopes: config.oidc_scopes.clone(),
        })
    }
}

#[async_trait::async_trait]
impl OidcClientTrait for OidcClient {
    fn authorization_url(&self) -> (openidconnect::url::Url, CsrfToken, Nonce) {
        let mut auth_request = self.inner.authorize_url(
            CoreAuthenticationFlow::AuthorizationCode,
            CsrfToken::new_random,
            Nonce::new_random,
        );
        for scope in &self.scopes {
            if scope != "openid" {
                auth_request = auth_request.add_scope(Scope::new(scope.clone()));
            }
        }
        auth_request.url()
    }

    async fn exchange_code(
        &self,
        code: &str,
        expected_nonce: &Nonce,
    ) -> Result<(String, String), AppError> {
        let client = self.http_client.clone();
        let exchange_fn = move |req: HttpRequest| {
            let client = client.clone();
            async move { send_http_request(&client, req).await }
        };

        let token_response = self
            .inner
            .exchange_code(AuthorizationCode::new(code.to_string()))
            .map_err(|e| {
                tracing::error!("VNL-AUTH-002: OIDC code exchange config error: {e}");
                AppError::OidcError(e.to_string())
            })?
            .request_async(&exchange_fn)
            .await
            .map_err(|e| {
                tracing::error!("VNL-AUTH-002: OIDC code exchange failed: {e:#}");
                AppError::OidcError(e.to_string())
            })?;

        let id_token = token_response.id_token().ok_or_else(|| {
            AppError::OidcError("VNL-AUTH-002: no id_token in response".to_string())
        })?;

        let id_token_claims = id_token
            .claims(&self.inner.id_token_verifier(), expected_nonce)
            .map_err(|e| {
                AppError::OidcError(format!("VNL-AUTH-003: token verification failed: {e}"))
            })?;

        let email = id_token_claims
            .email()
            .ok_or_else(|| {
                AppError::OidcError("VNL-AUTH-002: no email in token claims".to_string())
            })?
            .to_string();

        let id_token_string = id_token.to_string();
        Ok((id_token_string, email))
    }
}

#[cfg(test)]
#[derive(Debug)]
pub struct MockOidcClient;

#[cfg(test)]
#[async_trait::async_trait]
impl OidcClientTrait for MockOidcClient {
    fn authorization_url(&self) -> (openidconnect::url::Url, CsrfToken, Nonce) {
        unimplemented!("MockOidcClient::authorization_url")
    }

    async fn exchange_code(
        &self,
        _code: &str,
        _expected_nonce: &Nonce,
    ) -> Result<(String, String), AppError> {
        unimplemented!("MockOidcClient::exchange_code")
    }
}
