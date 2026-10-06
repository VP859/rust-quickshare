use anyhow::{Result, anyhow};
use base64::{Engine as _, engine::general_purpose::URL_SAFE_NO_PAD};
use iroh_net::NodeAddr;
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct NodeTicket {
    pub addr: NodeAddr,
}

impl NodeTicket {
    pub fn new(addr: NodeAddr) -> Self {
        Self { addr }
    }

    pub fn serialize_to_string(&self) -> Result<String> {
        let bytes = postcard::to_allocvec(self)?;
        Ok(URL_SAFE_NO_PAD.encode(bytes))
    }

    pub fn deserialize_from_str(s: &str) -> Result<Self> {
        let bytes = URL_SAFE_NO_PAD
            .decode(s.trim())
            .map_err(|e| anyhow!("Base64 decoding error: {}", e))?;
        let ticket: NodeTicket = postcard::from_bytes(&bytes)
            .map_err(|e| anyhow!("Postcard deserialization error: {}", e))?;
        Ok(ticket)
    }
}
