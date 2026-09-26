//! Blocking HTTP client for the upstream catalog service.

use std::time::Duration;

use serde::Deserialize;

use crate::error::{Error, Result};

/// Default per-request timeout.
pub const DEFAULT_TIMEOUT: Duration = Duration::from_secs(10);

/// A price quote for one SKU.
#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
pub struct Price {
    pub sku: String,
    pub cents: u64,
}

/// Units available for one SKU across warehouses.
#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
pub struct Inventory {
    pub sku: String,
    pub available: u32,
}

/// A product description from the catalog.
#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
pub struct Description {
    pub sku: String,
    pub title: String,
    pub body: String,
}

/// A client bound to one catalog base URL.
pub struct CatalogClient {
    base_url: String,
    agent: ureq::Agent,
}

impl CatalogClient {
    /// A client for `base_url` (without a trailing slash) using
    /// `DEFAULT_TIMEOUT` for every request.
    pub fn new(base_url: &str) -> CatalogClient {
        let agent = ureq::AgentBuilder::new().timeout(DEFAULT_TIMEOUT).build();
        CatalogClient { base_url: base_url.trim_end_matches('/').to_string(), agent }
    }

    /// The absolute URL for `path` under the base URL.
    pub fn endpoint(&self, path: &str) -> String {
        format!("{}/{}", self.base_url, path.trim_start_matches('/'))
    }

    /// Fetch the current price of `sku`.
    pub fn fetch_price(&self, sku: &str) -> Result<Price> {
        let url = self.endpoint(&format!("prices/{sku}"));
        let response = self
            .agent
            .get(&url)
            .call()
            .map_err(|e| Error::network(format!("GET {url}"), e))?;
        response
            .into_json()
            .map_err(|e| Error::decode(format!("decoding price from {url}"), e))
    }

    /// Fetch prices for many SKUs in one request, in the order the service
    /// returns them.
    pub fn fetch_prices(&self, skus: &[&str]) -> Result<Vec<Price>> {
        let url = self.endpoint("prices");
        let query = skus.join(",");
        let response = self
            .agent
            .get(&url)
            .query("skus", &query)
            .call()
            .map_err(|e| Error::network(format!("GET {url}?skus={query}"), e))?;
        response
            .into_json()
            .map_err(|e| Error::decode(format!("decoding prices from {url}?skus={query}"), e))
    }

    /// Fetch the product description for `sku`.
    pub fn fetch_description(&self, sku: &str) -> Result<Description> {
        let url = self.endpoint(&format!("descriptions/{sku}"));
        let response = self
            .agent
            .get(&url)
            .call()
            .map_err(|e| Error::network(format!("GET {url}"), e))?;
        response
            .into_json()
            .map_err(|e| Error::decode(format!("decoding description from {url}"), e))
    }

    /// Fetch how many units of `sku` are available.
    pub fn fetch_inventory(&self, sku: &str) -> Result<Inventory> {
        let url = self.endpoint(&format!("inventory/{sku}"));
        let response = self
            .agent
            .get(&url)
            .call()
            .map_err(|e| Error::network(format!("GET {}", self.endpoint("prices")), e))?;
        response
            .into_json()
            .map_err(|e| Error::decode(format!("decoding inventory from {url}"), e))
    }

    /// Report a sold quantity for `sku` so upstream inventory stays in sync.
    pub fn post_sale(&self, sku: &str, quantity: u32) -> Result<()> {
        let url = self.endpoint("sales");
        self.agent
            .post(&url)
            .send_json(ureq::json!({ "sku": sku, "quantity": quantity }))
            .map_err(|e| Error::network(format!("POST {url} (sku {sku}, quantity {quantity})"), e))?;
        Ok(())
    }

    /// Whether the service answers its health check with a 2xx status.
    /// Transport failures count as unhealthy rather than as errors.
    pub fn is_healthy(&self) -> bool {
        let url = self.endpoint("health");
        match self.agent.get(&url).call() {
            Ok(response) => (200..300).contains(&response.status()),
            Err(_) => false,
        }
    }
}

/// Everything the storefront shows for one SKU.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Listing {
    pub price: Price,
    pub inventory: Inventory,
    pub description: Description,
}

/// Fetch price, inventory, and description for `sku` and bundle them.
pub fn fetch_listing(client: &CatalogClient, sku: &str) -> Result<Listing> {
    Ok(Listing {
        price: client.fetch_price(sku)?,
        inventory: client.fetch_inventory(sku)?,
        description: client.fetch_description(sku)?,
    })
}
