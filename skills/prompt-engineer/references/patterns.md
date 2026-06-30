# Prompt Patterns Reference

## Zero-Shot Prompting

Direct instruction with no examples. Effective when:
- Task is well-defined and unambiguous
- Model has strong prior knowledge of the domain
- Output format is simple

```
You are a {{role}}. {{task_instruction}}

Output format: {{format_spec}}

Input: {{user_input}}
```

## Few-Shot Learning

Provide input→output examples to demonstrate the desired behavior.

**Example selection:**
- Choose 3-5 diverse examples covering different sub-cases
- Order from simple to complex
- Include at least one edge case
- Ensure format consistency across all examples
- Balance diversity (don't over-represent one category)

**Dynamic selection:** When the example pool is large, select examples most similar to the current input (nearest-neighbor by embedding or keyword overlap).

```
Task: {{task_description}}

Example 1:
Input: {{example_input_1}}
Output: {{example_output_1}}

Example 2:
Input: {{example_input_2}}
Output: {{example_output_2}}

Now process:
Input: {{user_input}}
Output:
```

## Chain-of-Thought (CoT)

Elicit step-by-step reasoning for complex problems.

**When to use:** Multi-step math, logic, planning, analysis, or any task where intermediate reasoning improves accuracy.

**Variants:**
- **Explicit CoT** — "Think step by step before answering"
- **Structured CoT** — Define numbered reasoning steps
- **Self-verification** — Add "Verify your answer" at the end

```
{{task_instruction}}

Think through this step by step:
1. Identify the key information
2. Break down the problem
3. Work through each step
4. Verify the result
5. State the final answer

Input: {{user_input}}
```

**Self-correction pattern:**
```
After generating your answer, review it:
- Does it satisfy all constraints?
- Are there any logical errors?
- If you find an error, correct it before responding.
```

## Tree-of-Thought (ToT)

Explore multiple reasoning paths and select the best.

**When to use:** Problems with multiple valid approaches, creative tasks, strategy/planning.

```
Consider this problem: {{problem}}

Generate 3 different approaches:

Approach 1: {{approach}}
- Pros: ...
- Cons: ...
- Confidence: X/10

Approach 2: ...
Approach 3: ...

Select the best approach and execute it fully.
```

## ReAct (Reasoning + Acting)

Interleave reasoning with tool use / actions.

**When to use:** Agentic workflows, tool-augmented generation, multi-step tasks requiring external data.

```
You have access to these tools: {{tool_list}}

For each step:
Thought: reason about what to do next
Action: tool_name(parameters)
Observation: [tool output]
... repeat until task complete ...
Final Answer: {{answer}}
```

## Constitutional AI / Self-Critique

Add output filtering through self-evaluation against principles.

**When to use:** Safety-critical outputs, bias-sensitive content, public-facing text.

```
{{task_instruction}}

After generating your response, evaluate it against these principles:
1. Is it harmful, misleading, or biased?
2. Does it respect user privacy?
3. Is it factually accurate?
4. Does it follow the requested format?

If any check fails, revise the response before outputting.
```

## Role-Based Prompting

Assign a specific persona/role to shape behavior and tone.

**When to use:** Consistent voice/expertise needed, domain-specific tasks, customer-facing outputs.

```
You are a {{role}} with expertise in {{domain}}.
Your communication style is {{style}}.
You always {{behavioral_constraint}}.
You never {{negative_constraint}}.

{{task_instruction}}
```

**Effective roles combine:**
- Domain expertise ("senior backend engineer")
- Behavioral traits ("meticulous about error handling")
- Constraints ("never suggest deprecated APIs")

## Instruction-Following Patterns

Maximize compliance with complex multi-part instructions:

```
You will receive a {{input_type}}. Follow these rules exactly:

RULES:
1. {{rule_1}}
2. {{rule_2}}
3. {{rule_3}}

OUTPUT FORMAT:
{{format_specification}}

CONSTRAINTS:
- {{constraint_1}}
- {{constraint_2}}

IMPORTANT: If the input violates any rule, respond with: "{{error_message}}"
```
