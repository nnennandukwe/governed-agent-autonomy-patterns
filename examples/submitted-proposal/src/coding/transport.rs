//! Bound inbound framing and outstanding RPC work before SDK handler dispatch.
use super::mcp::CodingServer;
use rmcp::{
    RoleServer,
    model::{ClientNotification, GetExtensions, JsonRpcMessage, RequestId},
    service::{RxJsonRpcMessage, TxJsonRpcMessage},
    transport::{Transport, async_rw::AsyncRwTransport},
};
use std::sync::{Arc, Mutex};

pub const MAX_FRAME_BYTES: usize = 256 * 1024;
pub const MAX_OUTSTANDING_REQUESTS: usize = 8;

pub fn stdio(server: CodingServer) -> impl Transport<RoleServer, Error = std::io::Error> {
    let (read, write) = rmcp::transport::stdio();
    Ingress {
        inner: AsyncRwTransport::new_server(read, write).with_max_line_length(MAX_FRAME_BYTES),
        outstanding: Arc::default(),
        server,
    }
}

struct PendingRequest {
    id: RequestId,
    handler_done: bool,
    cancelled: bool,
}
struct HandlerCompletion {
    id: RequestId,
    outstanding: Arc<Mutex<Vec<PendingRequest>>>,
}
impl Drop for HandlerCompletion {
    fn drop(&mut self) {
        let mut outstanding = self
            .outstanding
            .lock()
            .unwrap_or_else(|error| error.into_inner());
        if let Some(pending) = outstanding.iter_mut().find(|pending| pending.id == self.id) {
            pending.handler_done = true;
        }
        outstanding.retain(|pending| !(pending.cancelled && pending.handler_done));
    }
}
struct Ingress<T> {
    inner: T,
    outstanding: Arc<Mutex<Vec<PendingRequest>>>,
    server: CodingServer,
}

impl<T: Transport<RoleServer>> Transport<RoleServer> for Ingress<T> {
    type Error = T::Error;

    fn send(
        &mut self,
        message: TxJsonRpcMessage<RoleServer>,
    ) -> impl Future<Output = Result<(), Self::Error>> + Send + 'static {
        let id = match &message {
            JsonRpcMessage::Response(response) => Some(response.id.clone()),
            JsonRpcMessage::Error(error) => error.id.clone(),
            _ => None,
        };
        let outstanding = self.outstanding.clone();
        let sent = self.inner.send(message);
        async move {
            sent.await?;
            if let Some(id) = id {
                outstanding
                    .lock()
                    .unwrap_or_else(|e| e.into_inner())
                    .retain(|pending| pending.id != id);
            }
            Ok(())
        }
    }

    async fn receive(&mut self) -> Option<RxJsonRpcMessage<RoleServer>> {
        let Some(mut message) = self.inner.receive().await else {
            self.server.shutdown();
            eprintln!(
                "MCP connection closed; cancelling pending work and retaining actual outcomes."
            );
            return None;
        };
        if let JsonRpcMessage::Request(request) = &mut message {
            let mut outstanding = self.outstanding.lock().unwrap_or_else(|e| e.into_inner());
            if outstanding.iter().any(|pending| pending.id == request.id) {
                self.server.shutdown();
                eprintln!(
                    "MCP duplicate active request ID; closing the connection and draining pending work."
                );
                return None;
            }
            if outstanding.len() >= MAX_OUTSTANDING_REQUESTS {
                self.server.shutdown();
                eprintln!(
                    "MCP outstanding request limit exceeded; closing the connection and draining pending work."
                );
                return None;
            }
            outstanding.push(PendingRequest {
                id: request.id.clone(),
                handler_done: false,
                cancelled: false,
            });
            drop(outstanding);
            // The SDK moves this extension into the handler's RequestContext.
            // Its lifetime includes draining work whose response is suppressed.
            request
                .request
                .extensions_mut()
                .insert(Arc::new(HandlerCompletion {
                    id: request.id.clone(),
                    outstanding: self.outstanding.clone(),
                }));
        } else if let JsonRpcMessage::Notification(notification) = &message
            && let ClientNotification::CancelledNotification(cancelled) = &notification.notification
            && let Some(id) = &cancelled.params.request_id
        {
            let mut outstanding = self
                .outstanding
                .lock()
                .unwrap_or_else(|error| error.into_inner());
            if let Some(pending) = outstanding.iter_mut().find(|pending| pending.id == *id) {
                pending.cancelled = true;
            }
            outstanding.retain(|pending| !(pending.cancelled && pending.handler_done));
        }
        Some(message)
    }

    fn close(&mut self) -> impl Future<Output = Result<(), Self::Error>> + Send {
        self.inner.close()
    }
}
