use anyhow::{Result, anyhow};
use base64::{Engine as _, engine::general_purpose::URL_SAFE_NO_PAD};
use iroh::EndpointAddr;
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct NodeTicket {
    pub addr: EndpointAddr,
}

impl NodeTicket {
    pub fn new(addr: EndpointAddr) -> Self {
        Self { addr }
    }

    pub fn serialize_to_string(&self) -> Result<String> {
        let bytes = postcard::to_allocvec(self)?;
        Ok(URL_SAFE_NO_PAD.encode(bytes))
    }

    pub fn deserialize_from_str(s: &str) -> Result<Self> {
        let bytes = URL_SAFE_NO_PAD
            .decode(s.trim())
            .map_err(|e| anyhow!("Błąd dekodowania Base64: {}", e))?;
        let ticket: NodeTicket = postcard::from_bytes(&bytes)
            .map_err(|e| anyhow!("Błąd deserializacji Postcard: {}", e))?;
        Ok(ticket)
    }
}