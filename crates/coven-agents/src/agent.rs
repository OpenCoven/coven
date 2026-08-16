use std::{fmt, sync::Arc};

use serde::{Deserialize, Serialize};

use crate::{InputGuardrail, Model, OutputGuardrail, Tool, ToolDefinition};

#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
#[serde(transparent)]
pub struct AgentId(String);

impl AgentId {
    pub fn new(value: impl Into<String>) -> Self {
        Self(value.into())
    }

    pub fn as_str(&self) -> &str {
        &self.0
    }
}

impl fmt::Display for AgentId {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        self.0.fmt(formatter)
    }
}

impl From<&str> for AgentId {
    fn from(value: &str) -> Self {
        Self::new(value)
    }
}

impl From<String> for AgentId {
    fn from(value: String) -> Self {
        Self::new(value)
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Handoff {
    pub name: String,
    pub description: String,
    pub target: AgentId,
}

impl Handoff {
    pub fn new(
        name: impl Into<String>,
        description: impl Into<String>,
        target: impl Into<AgentId>,
    ) -> Self {
        Self {
            name: name.into(),
            description: description.into(),
            target: target.into(),
        }
    }
}

pub(crate) struct AgentTool<C>
where
    C: Sync,
{
    pub(crate) tool: Arc<dyn Tool<C>>,
    pub(crate) definition: ToolDefinition,
}

pub struct Agent<C>
where
    C: Sync,
{
    pub(crate) id: AgentId,
    pub(crate) name: String,
    pub(crate) instructions: String,
    pub(crate) model: Arc<dyn Model<C>>,
    pub(crate) tools: Vec<AgentTool<C>>,
    pub(crate) handoffs: Vec<Handoff>,
    pub(crate) input_guardrails: Vec<Arc<dyn InputGuardrail<C>>>,
    pub(crate) output_guardrails: Vec<Arc<dyn OutputGuardrail<C>>>,
}

impl<C> Agent<C>
where
    C: Sync,
{
    pub fn new(
        id: impl Into<AgentId>,
        name: impl Into<String>,
        instructions: impl Into<String>,
        model: Arc<dyn Model<C>>,
    ) -> Self {
        Self {
            id: id.into(),
            name: name.into(),
            instructions: instructions.into(),
            model,
            tools: Vec::new(),
            handoffs: Vec::new(),
            input_guardrails: Vec::new(),
            output_guardrails: Vec::new(),
        }
    }

    pub fn id(&self) -> &AgentId {
        &self.id
    }

    pub fn name(&self) -> &str {
        &self.name
    }

    pub fn instructions(&self) -> &str {
        &self.instructions
    }

    pub fn with_tool(mut self, tool: Arc<dyn Tool<C>>) -> Self {
        let definition = tool.definition();
        self.tools.push(AgentTool { tool, definition });
        self
    }

    pub fn with_handoff(mut self, handoff: Handoff) -> Self {
        self.handoffs.push(handoff);
        self
    }

    pub fn with_input_guardrail(mut self, guardrail: Arc<dyn InputGuardrail<C>>) -> Self {
        self.input_guardrails.push(guardrail);
        self
    }

    pub fn with_output_guardrail(mut self, guardrail: Arc<dyn OutputGuardrail<C>>) -> Self {
        self.output_guardrails.push(guardrail);
        self
    }
}
