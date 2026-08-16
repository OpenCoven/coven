use async_trait::async_trait;
use serde::{Deserialize, Serialize};
use serde_json::Value;

use crate::{AgentId, BoxError, ToolDefinition};

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ToolCall {
    pub id: String,
    pub name: String,
    pub arguments: Value,
}

impl ToolCall {
    pub fn new(id: impl Into<String>, name: impl Into<String>, arguments: Value) -> Self {
        Self {
            id: id.into(),
            name: name.into(),
            arguments,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct HandoffCall {
    pub name: String,
}

impl HandoffCall {
    pub fn new(name: impl Into<String>) -> Self {
        Self { name: name.into() }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct HandoffDefinition {
    pub name: String,
    pub description: String,
    pub target: AgentId,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum ModelAction {
    ToolCall(ToolCall),
    Handoff(HandoffCall),
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ModelResponse {
    pub assistant_message: Option<String>,
    pub actions: Vec<ModelAction>,
}

impl ModelResponse {
    pub fn final_output(output: impl Into<String>) -> Self {
        Self {
            assistant_message: Some(output.into()),
            actions: Vec::new(),
        }
    }

    pub fn actions(actions: Vec<ModelAction>) -> Self {
        Self {
            assistant_message: None,
            actions,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum RunItem {
    UserMessage {
        content: String,
    },
    AssistantMessage {
        agent: AgentId,
        content: String,
    },
    ToolCall {
        agent: AgentId,
        call: ToolCall,
    },
    ToolResult {
        agent: AgentId,
        call_id: String,
        tool: String,
        output: Value,
    },
    Handoff {
        from: AgentId,
        to: AgentId,
        name: String,
    },
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ModelRequest {
    pub agent_id: AgentId,
    pub agent_name: String,
    pub instructions: String,
    pub items: Vec<RunItem>,
    pub tools: Vec<ToolDefinition>,
    pub handoffs: Vec<HandoffDefinition>,
}

#[async_trait]
pub trait Model<C>: Send + Sync
where
    C: Sync,
{
    async fn generate(&self, request: ModelRequest, context: &C)
        -> Result<ModelResponse, BoxError>;
}
