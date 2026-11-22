# Estimated cost of the feedback repair study

**Experiment B has $0 in authorized provider spending and zero authorized provider calls.** At the quoted rates, even the most expensive token mix uses less than the proposed **$25** spending stop. The token limit caps the pre-tax charge sooner. The owner still has to decide whether to fund any trials and how much to authorize. The offline controls use existing local compute. Actual provider usage and invoice cost for the proposed trials remain unknown. [Study budget](budget.json) · [Maintainer decision](https://github.com/nnennandukwe/governed-agent-autonomy-patterns/issues/29#issuecomment-7071790843)

## Price and model

The estimate uses Anthropic's direct Messages API with the pinned model ID `claude-sonnet-5`, medium effort, adaptive thinking and standard global text pricing. Anthropic listed the model as active on November 21, 2026. Its standard price was **$2 per million input tokens and $10 per million output tokens**. These are list prices in US dollars. The estimate assumes no prompt caching, paid server tools, batch discount or regional premium. It excludes taxes, local hardware and staff time. [Model IDs](https://platform.claude.com/docs/en/about-claude/models/model-ids-and-versions) · [Model status](https://platform.claude.com/docs/en/about-claude/model-deprecations) · [Provider pricing](https://platform.claude.com/docs/en/about-claude/pricing)

The archived BoundaryBench pilot used this model and effort. Its price snapshot expired August 31, 2026. On August 10 Anthropic made the introductory rates standard and canceled the scheduled September price increase. The cost calculation here uses the November 21 check. [Archived pilot profile](https://github.com/nnennandukwe/governed-agent-autonomy-patterns/blob/6316a77d4033a269cfbd9162b16a67b62ffd1d47/boundarybench/experiment/pilot.config.example.json) · [Anthropic's price decision](https://platform.claude.com/docs/en/release-notes/overview#august-10-2026)

## Charges across all four strategies

Each of 12 families has one instance assigned once to each of the four strategies. All inputs, including feedback, replayed messages, tool definitions and tool results, count on every call. Billed output includes thinking and tool arguments. The combined limit of 12,000 tokens applies to the whole cell across both calls. At the quoted rates, one microdollar is one millionth of a dollar. A cell using 9,000 input and 3,000 output tokens costs `9,000 × 2 + 3,000 × 10 = 48,000` microdollars, or **$0.048**. This example illustrates a possible mix. [Resource decision and comparison design](https://github.com/nnennandukwe/governed-agent-autonomy-patterns/issues/29#issuecomment-7068231915)

| Split | Cells | Provider calls at most | Combined tokens at most | All input | Example mix | Upper charge |
| --- | ---: | ---: | ---: | ---: | ---: | ---: |
| Development | 8 | 16 | 96,000 | $0.192 | $0.384 | $0.96 |
| Validation | 8 | 16 | 96,000 | $0.192 | $0.384 | $0.96 |
| Held-out | 32 | 64 | 384,000 | $0.768 | $1.536 | $3.84 |
| **Total** | **48** | **96** | **576,000** | **$1.152** | **$2.304** | **$5.76** |

All three price columns assume full token use. The upper charge prices every token at the higher output rate. A cell that consumes fewer tokens can cost less than the all-input figure. Each cell includes up to two candidates, four strategy tool calls and one deterministic local evaluation within five minutes. The study allows up to 96 candidates, 192 strategy tool calls, 48 local evaluations and four hours of aggregate trial time. The evaluator adds zero provider calls. Development calibration, selection and retries consume the same limits. Automatic retries and subagents have zero allowance.

## Before the first paid request

The owner must approve a frozen trial plan and a monetary stop. Recheck model availability, service class, price and billing categories against the provider's current terms. Retain that dated evidence with the trial manifest. If the model, rate or service class changes, hold the trial and revise the estimate for the owner's decision. An unused balance under the proposed stop can't authorize a changed profile.

Reserve each call's input, maximum output, time and maximum charge from the cell and study balances before sending it. Keep the reservation for a timeout or an ambiguous response until provider records settle it. An increased spending limit can't establish what the provider billed. Stop new requests if the remaining budget can't cover the call, a charge is unknown, or a cap is reached. Preserve incomplete trials. Reconcile actual model IDs, request IDs and native input, cache and output usage with the account's billing records before reporting actual costs. The offline tests reproduce the arithmetic. They can't establish a provider bill or repair outcome.
