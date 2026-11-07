use thiserror::Error;

/// Fixed, user-safe phrases only. Never attach URLs, provider bodies, tokens or transport errors.
#[derive(Clone, Copy, Debug, Error, Eq, PartialEq)]
pub enum PluginError {
    #[error("This operation was cancelled. Check any submission outcome before trying again.")]
    Cancelled,
    #[error("The provider usage limit has been reached.")]
    UsageLimit,
    #[error("The setup token was refused. Obtain a new token from the provider.")]
    SetupTokenRefused,
    #[error("The provider requires an active subscription.")]
    ProviderSubscriptionRequired,
    #[error("This connection requires TLS encryption.")]
    TlsRequired,
    #[error("The provider certificate could not be verified.")]
    CertificateRejected,
    #[error("The outgoing message exceeds this connection's data limit.")]
    OutboundTooLarge,
    #[error("Submission may have occurred. Check the provider before sending again.")]
    DeliveryUnknown,
    #[error("This connection is not enabled for these books.")]
    Disabled,
    #[error("The connection settings are not valid.")]
    InvalidConfiguration,
    #[error("This plugin is already registered.")]
    DuplicatePlugin,
    #[error("This operation belongs to a different connection or company.")]
    WrongScope,
    #[error("Please sign in to this connection again.")]
    NeedsSignIn,
    #[error("The credential store could not complete the operation.")]
    CredentialStore,
    #[error("The plugin could not reach the provider.")]
    Network,
    #[error("This connection requires a secure HTTPS address.")]
    HttpsRequired,
    #[error("This connection is not allowed to contact that host.")]
    HostNotAllowed,
    #[error("Connections to private, local or reserved network addresses are not allowed.")]
    PrivateAddress,
    #[error("The provider tried to redirect the connection. Redirects are not allowed.")]
    Redirect,
    #[error("The transfer exceeds this connection's data limit.")]
    TooLarge,
    #[error("The connection took too long. Check its outcome before trying again.")]
    Timeout,
    #[error("The provider returned a response this connection could not read.")]
    InvalidResponse,
    #[error("The sign-in response did not match this sign-in attempt.")]
    OAuthState,
    #[error("The identity response did not match this sign-in attempt.")]
    OAuthNonce,
    #[error("The sign-in proof did not match this sign-in attempt.")]
    OAuthPkce,
    #[error("The sign-in callback used the wrong address or path.")]
    OAuthCallback,
    #[error("The sign-in was cancelled or refused.")]
    OAuthDenied,
    #[error("The plugin could not record its activity.")]
    Audit,
    #[error("The books could not complete this draft operation.")]
    Core,
}

pub type PluginResult<T> = Result<T, PluginError>;
