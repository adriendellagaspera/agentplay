# AgentPlay

**Agents as players for non-realtime games.**

AgentPlay is a runtime for multimodal agents to play games through the same visual observations and player-equivalent controls available to human players.

The core interaction model is deliberately small:

```text
observe -> think -> act -> settle -> observe
```

AgentPlay targets games that can be treated as a sequence of meaningful decision points. The agent may take arbitrarily long to reason between those points. It is **not** a runtime for continuous real-time control.

## Principles

### Agent as player

The agent should not receive semantic game APIs, object coordinates, rules, rewards, level identifiers, or privileged state unless a particular experiment explicitly opts into them. The default boundary is:

```text
visual observations in
player-equivalent inputs out
```

Private evaluator state may exist for scoring and reproducibility, but it must not leak across the agent boundary.

### Non-realtime by design

AgentPlay is intended for games where inference latency can be decoupled from game time: puzzle games, turn-based games, card games, tactics, many roguelikes, and similar environments.

A runtime step is not necessarily a fixed sleep. Different games may need different temporal policies:

- `FixedDelay`: wait a configured amount of time after an action.
- `UntilQuiescent`: observe frames until a meaningful visual state has settled.
- `FrozenStep`: advance a controllable clock for a bounded interval, then freeze again.
- `Manual`: let an environment-specific adapter decide when the next observation is actionable.

Only the policies needed by real games should be implemented; the abstraction exists so timing is explicit rather than hidden in ad-hoc sleeps.

### Runtime, not agent framework

AgentPlay owns environment interaction, synchronization, recording, replay, and budgets. It should not prescribe how an agent reasons, stores memory, builds world models, or calls a particular model provider.

TWIN-like, Tycho-like, frontier-model-native, scripted, and human agents should all be able to use the same environment trajectory.

## Initial target

The first end-to-end target is **Baba Is You** on macOS:

1. launch or attach to the real game;
2. capture only the game surface;
3. expose only controls available to a normal player, minus controls that can escape the sandbox;
4. wait for a stable/actionable state after each input;
5. record the exact observations and actions;
6. let a multimodal agent start from a fresh save and figure everything else out.

The game itself remains a black box to the agent.

## Architecture direction

```text
                      +------------------+
                      |      Agent       |
                      | Astra / human /  |
                      | Tycho / ...      |
                      +---------+--------+
                                |
                     observation / action
                                |
                     +----------v----------+
                     |   AgentPlay runner  |
                     | budgets / recording |
                     +----------+----------+
                                |
                          Environment
                                |
             +------------------+------------------+
             |                                     |
      temporal policy                      platform backend
   action -> settle -> frame          capture / input / process
             |                                     |
             +------------------+------------------+
                                |
                             Game
```

The native runtime is being built in Rust. Model/provider adapters are intentionally kept outside the environment core; a small process protocol and a Python/Gymnasium bridge are expected later for interoperability with research tooling.

## Scope

Good fits include:

- Baba Is You and Sokoban-like puzzle games;
- turn-based tactics such as Into the Breach;
- card/deck-building games such as Slay the Spire;
- turn-based roguelikes;
- non-realtime mobile games through a device/emulator backend;
- browser games with discrete decision points.

Out of scope for the initial runtime:

- FPS and racing games;
- fighting games;
- platformers requiring continuous control;
- RTS micro;
- environments where meaningful state changes continuously while the model reasons.

## Prior art

AgentPlay builds on ideas demonstrated by several recent systems while targeting a narrower missing runtime layer:

- **NitroGen / Universal Simulator**: commercial games exposed as Gymnasium environments with visual observations and virtual controller input; uses fixed-duration simulation stepping and is currently Windows-oriented.
- **LMGame / GamingAgent**: standardized Gym/Retro evaluation plus game-specific computer-use agents; a generic UI-only interface is announced but not publicly implemented at the time AgentPlay was started.
- **Cradle**: screenshot-in / keyboard-mouse-out agents for real software and games, with environment-specific adaptations.
- **VideoGameBench Lite**: pauses games while slow multimodal models reason.
- **GameWorld**: separates public UI interaction from private evaluation state.
- **TWIN and Tycho**: agent-side executable world models and active abstraction; complementary to, rather than part of, the environment runtime.

AgentPlay does **not** claim to invent pixels-to-input game agents. Its intended contribution is a lightweight, reusable runtime for turning opaque visual games into discrete environments for slow multimodal agents, with temporal synchronization treated as a first-class concern.

## Status

Early bootstrap. The first milestone is a human-controlled Baba Is You run through exactly the same `observe -> step -> settle` path that an agent will later use.
