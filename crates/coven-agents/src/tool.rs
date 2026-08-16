use async_trait::async_trait;
use serde::{Deserialize, Serialize};
use serde_json::Value;

use crate::BoxError;

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ToolDefinition {
    pub name: String,
    pub description: String,
    pub input_schema: Value,
}

impl ToolDefinition {
    pub fn new(
        name: impl Into<String>,
        description: impl Into<String>,
        input_schema: Value,
    ) -> Self {
        Self {
            name: name.into(),
            description: description.into(),
            input_schema,
        }
    }
}

#[async_trait]
pub trait Tool<C>: Send + Sync
where
    C: Sync,
{
    fn definition(&self) -> ToolDefinition;

    async fn execute(&self, arguments: Value, context: &C) -> Result<Value, BoxError>;
}
