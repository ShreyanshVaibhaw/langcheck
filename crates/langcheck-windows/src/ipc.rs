//! Same-user, local-only named-pipe transport for broker IPC (the post-MVP TSF
//! adapter <-> broker channel).
//!
//! The wire *protocol* (messages + encoding) lives in `langcheck-core::ipc`; this
//! module is only the Windows transport. Three safety properties are enforced
//! (`blueprint.md` Step 13: "same-user authenticated IPC", "reject remote
//! named-pipe clients", "inside the current device and user session"):
//!
//! - **Same user.** The pipe is created with a DACL granting access only to the
//!   creating user's SID (+ SYSTEM), and its name is namespaced by that SID. Both
//!   ends run as the same user, so both derive the same name.
//! - **Local only.** `PIPE_REJECT_REMOTE_CLIENTS` refuses any network client.
//! - **Bounded.** Fixed buffers; one message per exchange (message-mode pipe).
//!
//! The transport carries opaque bytes and contains no language logic.

use core::ffi::c_void;

use langcheck_core::ipc::{
    decode_request, decode_response, encode_request, encode_response, Request, Response,
};
use windows::core::{Error, Result, HRESULT, PCWSTR, PWSTR};
use windows::Win32::Foundation::{
    CloseHandle, LocalFree, ERROR_PIPE_CONNECTED, HANDLE, HLOCAL, INVALID_HANDLE_VALUE,
};
use windows::Win32::Security::Authorization::{
    ConvertSidToStringSidW, ConvertStringSecurityDescriptorToSecurityDescriptorW, SDDL_REVISION_1,
};
use windows::Win32::Security::{
    GetTokenInformation, TokenUser, PSECURITY_DESCRIPTOR, PSID, SECURITY_ATTRIBUTES, TOKEN_QUERY,
    TOKEN_USER,
};
use windows::Win32::Storage::FileSystem::{
    FlushFileBuffers, ReadFile, WriteFile, PIPE_ACCESS_DUPLEX,
};
use windows::Win32::System::Pipes::{
    CallNamedPipeW, ConnectNamedPipe, CreateNamedPipeW, DisconnectNamedPipe, PIPE_READMODE_MESSAGE,
    PIPE_REJECT_REMOTE_CLIENTS, PIPE_TYPE_MESSAGE, PIPE_UNLIMITED_INSTANCES, PIPE_WAIT,
};
use windows::Win32::System::Threading::{GetCurrentProcess, OpenProcessToken};

/// Pipe buffer size (bytes). Comfortably larger than any protocol message.
const BUF_SIZE: u32 = 1024;
/// Client connect/transact timeout (milliseconds).
const TIMEOUT_MS: u32 = 5_000;

fn err(message: &str) -> Error {
    Error::new(HRESULT(-1), message)
}

/// UTF-16, NUL-terminated copy of `s`.
fn wide(s: &str) -> Vec<u16> {
    s.encode_utf16().chain(std::iter::once(0)).collect()
}

/// The current user's SID as a string (e.g. `S-1-5-21-...`).
fn current_user_sid() -> Result<String> {
    let mut token = HANDLE::default();
    // SAFETY: pseudo-handle from GetCurrentProcess; `token` written on success.
    unsafe { OpenProcessToken(GetCurrentProcess(), TOKEN_QUERY, &mut token) }?;

    // First call sizes the buffer (expected to fail with ERROR_INSUFFICIENT_BUFFER).
    let mut needed = 0u32;
    // SAFETY: querying the required size with a null buffer.
    let _ = unsafe { GetTokenInformation(token, TokenUser, None, 0, &mut needed) };
    let mut buffer = vec![0u8; needed.max(1) as usize];
    // SAFETY: `buffer` is `needed` bytes; receives a TOKEN_USER.
    let queried = unsafe {
        GetTokenInformation(
            token,
            TokenUser,
            Some(buffer.as_mut_ptr().cast::<c_void>()),
            needed,
            &mut needed,
        )
    };
    // SAFETY: `token` is a valid open handle.
    unsafe {
        let _ = CloseHandle(token);
    }
    queried?;

    // SAFETY: `buffer` holds a TOKEN_USER; read the contained SID pointer.
    let sid: PSID = unsafe { (*buffer.as_ptr().cast::<TOKEN_USER>()).User.Sid };
    let mut wide_sid = PWSTR::null();
    // SAFETY: `sid` is valid for the call; `wide_sid` receives a LocalAlloc'd string.
    unsafe { ConvertSidToStringSidW(sid, &mut wide_sid) }?;
    // SAFETY: `wide_sid` is a valid NUL-terminated wide string on success.
    let text = unsafe { wide_sid.to_string() }.map_err(|_| err("invalid SID string"));
    // SAFETY: free the LocalAlloc'd SID string.
    unsafe {
        let _ = LocalFree(HLOCAL(wide_sid.0.cast::<c_void>()));
    }
    text
}

/// Per-user pipe name, namespaced by the current user's SID.
fn pipe_name() -> Result<Vec<u16>> {
    let sid = current_user_sid()?;
    Ok(wide(&format!(r"\\.\pipe\LangCheck.broker.{sid}")))
}

/// A bound broker-side pipe instance that answers one request per connection.
pub struct PipeServer {
    handle: HANDLE,
}

// SAFETY: the contained HANDLE is owned solely by this struct and only used from
// whichever single thread holds it; transferring ownership to a server thread is
// sound (no shared access, and a pipe handle is a kernel object valid cross-thread).
unsafe impl Send for PipeServer {}

impl PipeServer {
    /// Create the per-user pipe with a same-user DACL and remote clients rejected.
    pub fn bind() -> Result<Self> {
        let name = pipe_name()?;
        let sid = current_user_sid()?;
        // Protected DACL: GENERIC_ALL to this user and to SYSTEM, nobody else.
        let sddl = wide(&format!("D:P(A;;GA;;;{sid})(A;;GA;;;SY)"));

        let mut descriptor = PSECURITY_DESCRIPTOR::default();
        // SAFETY: `sddl` is a valid wide SDDL string; `descriptor` receives a
        // LocalAlloc'd security descriptor on success.
        unsafe {
            ConvertStringSecurityDescriptorToSecurityDescriptorW(
                PCWSTR(sddl.as_ptr()),
                SDDL_REVISION_1,
                &mut descriptor,
                None,
            )
        }?;

        let attributes = SECURITY_ATTRIBUTES {
            nLength: std::mem::size_of::<SECURITY_ATTRIBUTES>() as u32,
            lpSecurityDescriptor: descriptor.0,
            bInheritHandle: false.into(),
        };
        // SAFETY: `name` is a valid wide pipe name; `attributes` references the
        // descriptor built above, valid for the duration of the call.
        let handle = unsafe {
            CreateNamedPipeW(
                PCWSTR(name.as_ptr()),
                PIPE_ACCESS_DUPLEX,
                PIPE_TYPE_MESSAGE | PIPE_READMODE_MESSAGE | PIPE_WAIT | PIPE_REJECT_REMOTE_CLIENTS,
                PIPE_UNLIMITED_INSTANCES,
                BUF_SIZE,
                BUF_SIZE,
                0,
                Some(&attributes),
            )
        };
        // SAFETY: `descriptor` was allocated by the Convert call; the pipe captured
        // its own copy, so we free ours now.
        unsafe {
            let _ = LocalFree(HLOCAL(descriptor.0));
        }

        if handle.is_invalid() || handle == INVALID_HANDLE_VALUE {
            return Err(Error::from_win32());
        }
        Ok(Self { handle })
    }

    /// Wait for one client, read its request, run `handler`, send the response,
    /// and reset the instance for the next client.
    pub fn serve_one<F: FnOnce(Request) -> Response>(&self, handler: F) -> Result<()> {
        // SAFETY: `handle` is a valid pipe; no overlapped I/O. ERROR_PIPE_CONNECTED
        // means a client connected between CreateNamedPipe and now — that is success.
        match unsafe { ConnectNamedPipe(self.handle, None) } {
            Ok(()) => {}
            Err(e) if e.code() == ERROR_PIPE_CONNECTED.to_hresult() => {}
            Err(e) => return Err(e),
        }
        let result = self.exchange(handler);
        // SAFETY: always drop the client connection, even on a handler/IO error,
        // so the next serve_one starts clean.
        unsafe {
            let _ = FlushFileBuffers(self.handle);
            let _ = DisconnectNamedPipe(self.handle);
        }
        result
    }

    fn exchange<F: FnOnce(Request) -> Response>(&self, handler: F) -> Result<()> {
        let mut buffer = [0u8; BUF_SIZE as usize];
        let mut read = 0u32;
        // SAFETY: `buffer` is a valid sized buffer; message-mode read of one message.
        unsafe { ReadFile(self.handle, Some(&mut buffer), Some(&mut read), None) }?;
        let request = decode_request(&buffer[..read as usize]).map_err(|e| err(&e.to_string()))?;
        let response = encode_response(&handler(request));
        let mut written = 0u32;
        // SAFETY: `response` is a valid buffer written as one message.
        unsafe { WriteFile(self.handle, Some(&response), Some(&mut written), None) }?;
        Ok(())
    }
}

impl Drop for PipeServer {
    fn drop(&mut self) {
        // SAFETY: `handle` is a valid pipe handle owned by this struct.
        unsafe {
            let _ = CloseHandle(self.handle);
        }
    }
}

/// Client side: send one request to the broker and read the response, in a single
/// transacted call. Runs as the same user, so it derives the same pipe name.
pub fn request(request: &Request) -> Result<Response> {
    let name = pipe_name()?;
    let payload = encode_request(request);
    let mut buffer = [0u8; BUF_SIZE as usize];
    let mut read = 0u32;
    // SAFETY: `name` is a valid wide pipe name; the in/out buffers are valid for
    // their stated sizes; `read` receives the response length.
    unsafe {
        CallNamedPipeW(
            PCWSTR(name.as_ptr()),
            Some(payload.as_ptr().cast::<c_void>()),
            payload.len() as u32,
            Some(buffer.as_mut_ptr().cast::<c_void>()),
            BUF_SIZE,
            &mut read,
            TIMEOUT_MS,
        )
        .ok()?
    };
    decode_response(&buffer[..read as usize]).map_err(|e| err(&e.to_string()))
}

#[cfg(test)]
mod tests {
    use super::*;
    use langcheck_core::session::Boundary;

    fn handler(request: Request) -> Response {
        match request {
            Request::Ping => Response::Pong,
            Request::Evaluate { token, .. } if token == "wierd" => Response::Replace {
                replacement: "weird".to_owned(),
            },
            Request::Evaluate { .. } => Response::Leave,
        }
    }

    #[test]
    fn round_trip_over_a_real_pipe() {
        let server = PipeServer::bind().expect("bind pipe");
        let server_thread = std::thread::spawn(move || {
            server.serve_one(handler).expect("serve ping");
            server.serve_one(handler).expect("serve evaluate");
        });

        let pong = request(&Request::Ping).expect("ping round-trip");
        assert_eq!(pong, Response::Pong);

        let reply = request(&Request::Evaluate {
            token: "wierd".to_owned(),
            boundary: Boundary::Space,
        })
        .expect("evaluate round-trip");
        assert_eq!(
            reply,
            Response::Replace {
                replacement: "weird".to_owned()
            }
        );

        server_thread.join().expect("server thread");
    }
}
