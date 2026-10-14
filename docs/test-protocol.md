# Test protocol

How to prove a pad is good enough before moving on. Every test runs from the serial console ([firmware/README.md](../firmware/README.md)).

**Contents:** [Single pad](#single-pad-gate-1) · [Two pads](#two-pad-distribution-phase-1b) · [Tips](#tips)

Targets scale with the pad's rating (`fs KG`, default 200). For corner balancing, **repeatability matters more than absolute accuracy**.

## Single pad (gate 1)

Tare the empty pad before each test.

| Test | Console | How | Pass at 200 kg |
| --- | --- | --- | --- |
| Zero noise | `test noise` | 100 readings, pad empty | std dev ≤ 50 g |
| Linearity | `cal add …` ×5+, `cal fit` | 0 → max in 5+ known steps | max residual ≤ 500 g |
| Repeatability | `test repeat` | Place and remove the same ~20 kg 10× | spread ≤ 200 g |
| Off-center | `test offcenter` | Same weight at center and each corner | within 0.5 % |
| Creep | `test creep 600` | Hold a heavy load 10 min | drift ≤ 200 g |
| Return to zero | `test zero` | Unload after creep | within 50 g |
| Temperature | `stream on` | Zero + one weight, cold vs. warm garage | record; set a re-tare rule |

**If off-center fails, the problem is mechanical** (binding mounts, flexing or touching plate). Fix it before phase 2; a tire never lands dead center.

Log any run with `stream on` and save the monitor output as CSV.

## Two-pad distribution (phase 1b)

Each pad first passes gate 1. Then lay a stiff plank across both pads, supports a span **L** apart, tare with the plank on, and move a known weight **W** along it in 5–7 steps.

With the weight at distance **x** from pad A's support (lever rule):

- Pad A reads **W · (L − x) / L**
- Pad B reads **W · x / L**

| Test | Pass |
| --- | --- |
| Split: pad B's share vs. x / L | within 0.2 percentage points |
| Sum: A + B vs. W on one pad | within 200 g |
| Link: stream both for 10 min | no gap over 1 s |

Two supports can't show leveling errors (the split barely moves when one pad sits higher). That needs four pads, in phase 3.

## Tips

- Weigh every reference weight on a known-good scale first.
- Let the pad warm up 10 minutes after power-on before measuring.
- Cheap cells vary: test each one before installing it.
