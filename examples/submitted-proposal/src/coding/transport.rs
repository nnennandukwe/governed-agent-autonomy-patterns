//! Bound inbound framing and outstanding RPC work before SDK handler dispatch.
use super::mcp::CodingServer;
use rmcp::{
    RoleServer,
    model::{JsonRpcMessage, RequestId},
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

struct Ingress<T> {
    inner: T,
    outstanding: Arc<Mutex<Vec<RequestId>>>,
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
                    .retain(|pending| pending != &id);
            }
            Ok(())
        }
    }

    async fn receive(&mut self) -> Option<RxJsonRpcMessage<RoleServer>> {
        let Some(message) = self.inner.receive().await else {
            self.server.shutdown();
            eprintln!(
                "MCP connection closed; cancelling pending work and retaining actual outcomes."
            );
            return None;
        };
        if let JsonRpcMessage::Request(request) = &message {
            let mut outstanding = self.outstanding.lock().unwrap_or_else(|e| e.into_inner());
            if outstanding.contains(&request.id) {
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
            outstanding.push(request.id.clone());
        }
        Some(message)
    }

    fn close(&mut self) -> impl Future<Output = Result<(), Self::Error>> + Send {
        self.inner.close()
    }
}
