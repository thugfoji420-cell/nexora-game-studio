# Nexora Game Studio

**Local-first visual production studio: Prompt → Image Generation → Hunyuan3D → Blender Processing → Interactive Review → User Approval → Unity Delivery**

Nexora Game Studio is a Tauri 2 desktop application that provides a complete local-first pipeline for generating, processing, and delivering 3D assets into Unity projects. Built with a Rust backend and React/TypeScript frontend, it emphasizes privacy, offline capability, and free/local tooling over cloud dependencies.

## Vision

```
Prompt → Image Generation (A1111/ComfyUI) → Hunyuan3D (local) → Blender Cleanup/Repair/Optimization/LODs
                                                      ↓
                                              GLB Validation (Three.js Viewer)
                                                      ↓
                                              Interactive Review: Approve / Reject / Reprocess
                                                      ↓
                                              Approved-Only Unity Delivery (multi-project targets)
```

## Core Philosophy

- **Local-first / Free-first**: All AI runs locally via Automatic1111, ComfyUI, and Hunyuan3D. No mandatory cloud APIs, no API keys required for core workflow.
- **Privacy by design**: Assets never leave your machine unless you explicitly configure external providers.
- **Offline-capable**: Core pipeline works without internet after initial model downloads.
- **Deterministic & auditable**: SQLite project database tracks every job, asset, checksum, and provenance.

## Implemented Features

| Area | Status | Details |
|------|--------|---------|
| **Tauri 2 + Rust Backend** | ✅ Complete | Native desktop shell, secure IPC, system dialogs |
| **React 19 + TypeScript Frontend** | ✅ Complete | Vite, Three.js viewer, Vitest test suite (66 tests) |
| **SQLite Project Architecture** | ✅ Complete | 10 migrations, WAL mode, project-scoped databases |
| **Automatic1111 Integration** | ✅ Complete | txt2img/img2img, loopback-only enforcement, strict validation |
| **ComfyUI Integration** | ✅ Complete | Wan 2.1 video generation, history polling, managed asset promotion |
| **Hunyuan3D Integration** | ✅ Complete | Local model execution via CLI, capability-based provider registry |
| **Blender Automated Processing** | ✅ Complete | Python mesh processor (cleanup, repair, optimize, LODs, GLB export) |
| **GLB Validation** | ✅ Complete | Structural validation, accessor/layout checks, animation counts |
| **Three.js Interactive 3D Viewer** | ✅ Complete | Orbit controls, wireframe toggle, LOD inspection, GLB drag-drop |
| **Approve / Reject / Reprocess Workflow** | ✅ Complete | Asset states: `staged` → `pending_review` → `approved`/`rejected` |
| **Local AI Runtime Manager** | ✅ Complete | Auto-discovery of A1111, ComfyUI, Hunyuan3D, Blender; health checks |
| **Multi-Project Unity Target Architecture** | ✅ Complete | Named targets, path validation, traversal protection |
| **Approved-Only Unity Delivery** | ✅ Complete | Atomic copy with manifest, checksum verification, idempotent |
| **Job Queue & Recovery** | ✅ Complete | Persistent queue, claim semantics, cancellation, retry bounds |
| **Provider Registry & Diagnostics** | ✅ Complete | Capability schema, compatibility matrix, health monitoring |

## Work In Progress / Planned

- [ ] Unity Editor package for seamless asset import (currently file-system delivery only)
- [ ] Hunyuan3D multi-GPU / batch optimization
- [ ] Advanced LOD generation presets per asset type
- [ ] Cloud provider adapters (optional, opt-in)
- [ ] Collaborative project sharing (future)

## Requirements

### Runtime Dependencies (User-Installed)

| Tool | Purpose | Minimum Version |
|------|---------|-----------------|
| **Automatic1111 WebUI** | Image generation (txt2img/img2img) | Any recent |
| **ComfyUI** | Video generation (Wan 2.1) | 0.33.0+ |
| **Hunyuan3D CLI** | 3D model generation from images | Tencent/Hunyuan3D-2 |
| **Blender** | Mesh processing, cleanup, LODs, GLB export | 4.0+ |
| **Unity** | Target game engine for delivery | 2022.3 LTS+ |

### Development Dependencies

- **Node.js** 20+ (LTS recommended)
- **Rust** 1.80+ (stable)
- **pnpm** or npm (for frontend)
- **Git**

> **Note**: Nexora does not bundle or auto-install AI runtimes. The Runtime Manager *detects* existing installations. You must install and configure A1111/ComfyUI/Hunyuan3D/Blender separately.

## Development Setup

```powershell
# Clone and enter
git clone <repository-url>
cd nexora-game-studio

# Frontend dependencies
npm install

# TypeScript type checking
npm run typecheck

# Frontend unit tests (66 tests)
npm test

# Rust backend checks
cd src-tauri
cargo check        # Compile check (warnings expected for unused items)
cargo test         # 144 backend tests (2 ignored = require live AI runtimes)

# Development server (hot reload)
cd ..
npm run tauri dev

# Production build
npm run tauri build
```

### Useful Commands

| Command | Description |
|---------|-------------|
| `npm run typecheck` | Strict TypeScript compilation (no emit) |
| `npm test` | Vitest unit tests (headless) |
| `npm run build` | TypeScript + Vite production build |
| `npm run tauri dev` | Tauri dev mode with Vite HMR |
| `npm run tauri build` | Full MSI/NSIS installer build |
| `cargo check` | Rust compile check (fast) |
| `cargo test` | Rust unit/integration tests |
| `cargo clippy` | Linting (pedantic/nursery) |

## Architecture Overview

```
┌─────────────────────────────────────────────────────────────────┐
│                        Nexora Game Studio                       │
├─────────────────────────────────────────────────────────────────┤
│  Frontend (React 19 + TS + Three.js)                           │
│  ├── Pages: Projects, Assets, Generators, Jobs, Review, Settings│
│  ├── Components: Model3dViewer, AssetVideoPreview, Sidebar     │
│  └── Services: Typed IPC wrappers for all backend commands     │
├─────────────────────────────────────────────────────────────────┤
│  Tauri 2 IPC Bridge (secure, capability-based)                 │
├─────────────────────────────────────────────────────────────────┤
│  Backend (Rust 2024 Edition)                                    │
│  ├── Project & Asset Management (SQLite + WAL)                 │
│  ├── Job Queue (persistent, recoverable, cancellable)          │
│  ├── Provider Registry (A1111, ComfyUI, Hunyuan3D, Blender)    │
│  ├── Runtime Manager (discovery, health, capability matching)  │
│  ├── Image Generation (A1111 loopback HTTP, strict validation) │
│  ├── Video Generation (ComfyUI history polling)                │
│  ├── 3D Generation (Hunyuan3D CLI orchestration)               │
│  ├── 3D Import/Validation/Processing (GLB, Blender Python)     │
│  ├── Asset Delivery (Unity targets, atomic copy, manifests)    │
│  └── Settings & Hardware Detection (CIM/NVIDIA-SMI)            │
└─────────────────────────────────────────────────────────────────┘
```

### Data Flow

1. **Project Creation** → SQLite database + manifest.json in user-chosen directory
2. **Image Generation** → A1111 API → validated PNG/JPEG → managed asset (checksum, provenance)
3. **3D Generation** → Hunyuan3D CLI (image → GLB) → validation → managed asset
4. **3D Processing** → Blender Python (cleanup, repair, optimize, LODs) → validated GLB → managed asset
5. **Interactive Review** → Three.js viewer → user action (approve/reject/reprocess)
6. **Unity Delivery** → Approved assets only → atomic copy to Unity target → manifest.json

### Security Boundaries

- All AI provider URLs restricted to numeric loopback (127.0.0.1/::1), explicit port, no credentials
- File operations confined to project root (canonicalization + traversal checks)
- Unity delivery destinations validated against traversal
- No secrets in provider manifests (protocol is descriptor-only)
- Only internal mock providers can be enabled for testing

## Project Status

| Component | Version | Maturity |
|-----------|---------|----------|
| Tauri Shell | 2.8.x | Stable |
| Frontend | React 19, Vite 7 | Stable |
| Backend | Rust 2024, Tauri 2 | Stable |
| Database Schema | 10 migrations | Stable |
| Test Coverage | 66 FE + 144 BE tests | Comprehensive |
| AI Provider Adapters | A1111, ComfyUI, Hunyuan3D | Functional |
| Blender Processor | Python 3.10+ | Functional |
| Unity Delivery | File-system + manifest | Functional |

**Version**: 0.1.0 (pre-1.0, active development)

## Contributing

We welcome contributions! Please read our guidelines:

1. **Fork & branch** from `main`
2. **Run checks** before PR:
   ```powershell
   npm run typecheck && npm test
   cd src-tauri && cargo check && cargo test
   ```
3. **Follow conventions**:
   - Rust: `cargo fmt`, `cargo clippy -- -D warnings` (warnings allowed for WIP)
   - TypeScript: ESLint + Prettier (configured)
   - Commits: Conventional Commits (`feat:`, `fix:`, `refactor:`, `test:`, `docs:`)
4. **No secrets** in code or config — use `.env.example` for templates
5. **Test locally** with real AI runtimes for integration paths

### Areas Needing Help

- Unity Editor package (C# / UPM)
- Additional 3D format support (FBX, USDZ)
- Accessibility improvements in viewer
- Documentation & tutorials
- Windows/Linux/macOS packaging verification

## License

**Apache License 2.0** — see [LICENSE](LICENSE) for full text.

```
Copyright 2026 Nexora

Licensed under the Apache License, Version 2.0 (the "License");
you may not use this file except in compliance with the License.
You may obtain a copy at http://www.apache.org/licenses/LICENSE-2.0
```

### Third-Party Notices

- Tauri, React, Three.js, rusqlite, serde, tokio, reqwest, and all transitive dependencies retain their respective licenses (MIT, Apache-2.0, BSD-3-Clause, etc.)
- Hunyuan3D model weights subject to Tencent's license
- Blender is GPL-3.0 (invoked as subprocess, not linked)

---

**Nexora Game Studio** — Building the local-first future of game asset production.