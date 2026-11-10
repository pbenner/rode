//! Kerberos user authentication (RFC 4462), using the existing ticket cache.
//! The optional system library is loaded only in the SSH connection process.
use super::*;

#[cfg(target_os = "linux")]
mod ffi;

#[cfg(target_os = "linux")]
const KRB5_DER: &[u8] = b"\x06\x09\x2a\x86\x48\x86\xf7\x12\x01\x02\x02";
#[cfg(target_os = "linux")]
const TOKEN_LIMIT: usize = 128 * 1024;

/// Authenticate with existing Kerberos credentials using a completed mutual/integrity context.
/// Credential acquisition runs on a blocking worker; delegation follows the captured SSH setting.
pub(super) async fn authenticate(
    session: &mut Session,
    config: &config::Config,
) -> Result<client::AuthResult> {
    #[cfg(target_os = "linux")]
    {
        let host = config.host.clone();
        let delegate = config.gssapi_delegate;
        // Acquire credentials and the initial service token before starting SSH
        // userauth. Missing/expired tickets then leave no unfinished exchange.
        let (context, first) = tokio::task::spawn_blocking(move || {
            let mut context = ffi::Context::new(&host, delegate)?;
            let first = context.step(None)?;
            Ok::<_, anyhow::Error>((context, first))
        })
        .await??;
        let mut auth = Authenticator {
            context: Some(context),
            first: Some(first),
            steps: 0,
            mic_ready: false,
        };
        let result = session
            .authenticate_gssapi_with_mic(&config.user, vec![KRB5_DER.to_vec()], &mut auth)
            .await?;
        require_completed_context(&result, auth.mic_ready)?;
        Ok(result)
    }
    #[cfg(not(target_os = "linux"))]
    {
        let _ = (session, config);
        bail!("Native Kerberos authentication is currently supported on Linux; select OpenSSH")
    }
}

#[cfg(target_os = "linux")]
fn require_completed_context(result: &client::AuthResult, mic_ready: bool) -> Result<()> {
    if result.success()
        || matches!(
            result,
            client::AuthResult::Failure {
                partial_success: true,
                ..
            }
        )
    {
        ensure!(
            mic_ready,
            "SSH server accepted Kerberos before mutual authentication completed"
        );
    }
    Ok(())
}

#[cfg(target_os = "linux")]
struct Authenticator {
    context: Option<ffi::Context>,
    first: Option<Vec<u8>>,
    steps: usize,
    mic_ready: bool,
}

#[cfg(target_os = "linux")]
impl russh::GssapiAuthenticator for Authenticator {
    type Error = anyhow::Error;

    async fn gssapi_error(&mut self, error: russh::GssapiError) {
        if let russh::GssapiError::Status { message, .. } = error {
            eprintln!("Kerberos server: {}", auth::visible(&message));
        }
        // Error tokens may contain credentials; never put them in login output.
    }

    async fn gssapi_step(
        &mut self,
        selected: Option<Vec<u8>>,
        input: Option<Vec<u8>>,
        mic_data: Vec<u8>,
    ) -> Result<russh::GssapiStep> {
        self.steps += 1;
        ensure!(self.steps <= 16, "Too many Kerberos authentication steps");
        ensure!(
            input
                .as_ref()
                .is_none_or(|token| token.len() <= TOKEN_LIMIT)
                && mic_data.len() <= TOKEN_LIMIT,
            "Kerberos authentication token exceeds limit"
        );
        if self.first.is_some() {
            ensure!(
                selected.as_deref() == Some(KRB5_DER) && input.is_none(),
                "SSH server selected an unoffered Kerberos mechanism"
            );
        } else {
            ensure!(selected.is_none(), "Repeated Kerberos mechanism selection");
            ensure!(input.is_some(), "Missing Kerberos continuation token");
        }
        let mut context = self.context.take().context("Kerberos context ended")?;
        let first = self.first.take();
        let (context, result) = tokio::task::spawn_blocking(move || {
            let result = (|| {
                let token = match first {
                    Some(token) => token,
                    None => context.step(input.as_deref())?,
                };
                if context.complete() {
                    Ok(russh::GssapiStep::Complete {
                        token: (!token.is_empty()).then_some(token),
                        mic: Some(context.mic(&mic_data)?),
                    })
                } else {
                    ensure!(!token.is_empty(), "Empty Kerberos continuation token");
                    Ok(russh::GssapiStep::Continue { token })
                }
            })();
            (context, result)
        })
        .await?;
        self.context = Some(context);
        self.mic_ready = matches!(
            &result,
            Ok(russh::GssapiStep::Complete { mic: Some(_), .. })
        );
        result
    }
}

#[cfg(all(test, target_os = "linux"))]
mod tests {
    use super::*;
    use russh::GssapiAuthenticator;

    #[tokio::test]
    async fn rejects_unoffered_mechanisms_repeated_selection_and_unbounded_exchanges() {
        let mut auth = Authenticator {
            context: None,
            first: Some(vec![1]),
            steps: 0,
            mic_ready: false,
        };
        let wrong = auth
            .gssapi_step(Some(vec![6, 0]), None, vec![])
            .await
            .unwrap_err();
        assert!(wrong.to_string().contains("unoffered"));
        assert!(auth.first.is_some());
        let oversized = auth
            .gssapi_step(Some(KRB5_DER.to_vec()), None, vec![0; TOKEN_LIMIT + 1])
            .await
            .unwrap_err();
        assert!(oversized.to_string().contains("exceeds limit"));
        auth.first = None;
        let repeated = auth
            .gssapi_step(Some(KRB5_DER.to_vec()), Some(vec![]), vec![])
            .await
            .unwrap_err();
        assert!(repeated.to_string().contains("Repeated"));
        let absent = auth.gssapi_step(None, None, vec![]).await.unwrap_err();
        assert!(absent.to_string().contains("Missing"));
        auth.steps = 16;
        let endless = auth
            .gssapi_step(None, Some(vec![]), vec![])
            .await
            .unwrap_err();
        assert!(endless.to_string().contains("Too many"));
    }

    #[test]
    fn server_cannot_claim_success_before_mutual_authentication() {
        let success = client::AuthResult::Success;
        let partial = client::AuthResult::Failure {
            partial_success: true,
            remaining_methods: russh::MethodSet::empty(),
        };
        for result in [success, partial] {
            assert!(require_completed_context(&result, false).is_err());
            assert!(require_completed_context(&result, true).is_ok());
        }
        assert!(
            require_completed_context(
                &client::AuthResult::Failure {
                    partial_success: false,
                    remaining_methods: russh::MethodSet::empty(),
                },
                false
            )
            .is_ok()
        );
    }
}
