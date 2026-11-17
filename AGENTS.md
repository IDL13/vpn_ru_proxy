# VPN RU Proxy

Adapted from the [rust-skills instructions for Codex](https://github.com/actionbook/rust-skills/blob/main/AGENTS.md).

## Project checks

- Use Rust 2024 and keep compatibility with Rust 1.89, which provides the file-locking API this project uses.
- Use standard Rust naming and formatting. Propagate recoverable errors with `?` and explain the operation when the underlying error lacks context.
- Document every `unsafe` block with a `// SAFETY:` comment that explains the invariants for that call.
- Run `cargo fmt --all -- --check`, `cargo clippy --locked --all-targets -- -D warnings`, and `cargo test --locked`. Run `scripts/smoke-test.py` with sing-box when changing the Chrome protocol or proxy lifecycle.

## Core Capabilities

### 1. Question Routing
Route Rust questions to appropriate skills:
- Ownership/borrowing → m01-ownership
- Smart pointers → m02-resource
- Error handling → m06-error-handling
- Concurrency → m07-concurrency
- Unsafe code → unsafe-checker

### 2. Code Style
Follow Rust coding guidelines:
- Use snake_case for variables and functions
- Use PascalCase for types and traits
- Use SCREAMING_SNAKE_CASE for constants
- Max line length: 100 characters
- Use `?` operator instead of `unwrap()` in library code

### 3. Error Handling
```rust
// Good: Use Result with context
fn read_config() -> Result<Config, ConfigError> {
    let content = std::fs::read_to_string("config.toml")
        .map_err(|e| ConfigError::Io(e))?;
    toml::from_str(&content)
        .map_err(|e| ConfigError::Parse(e))
}

// Avoid: unwrap() in library code
fn read_config() -> Config {
    let content = std::fs::read_to_string("config.toml").unwrap(); // Bad
    toml::from_str(&content).unwrap() // Bad
}
```

### 4. Unsafe Code
Every `unsafe` block MUST have a `// SAFETY:` comment:
```rust
// SAFETY: We checked that index < len above, so this is in bounds
unsafe { slice.get_unchecked(index) }
```

### 5. Common Error Fixes

| Error | Cause | Fix |
|-------|-------|-----|
| E0382 | Use of moved value | Clone, borrow, or use reference |
| E0597 | Lifetime too short | Extend lifetime or restructure |
| E0502 | Borrow conflict | Split borrows or use RefCell |
| E0499 | Multiple mut borrows | Restructure to single mut borrow |
| E0277 | Missing trait impl | Add trait bound or implement trait |

## Quick Reference

### Ownership
- Each value has one owner
- Borrowing: `&T` (shared) or `&mut T` (exclusive)
- Lifetimes: `'a` annotations for references

### Smart Pointers
- `Box<T>`: Heap allocation
- `Rc<T>`: Reference counting (single-threaded)
- `Arc<T>`: Atomic reference counting (thread-safe)
- `RefCell<T>`: Interior mutability

### Concurrency
- `Send`: Safe to transfer between threads
- `Sync`: Safe to share references between threads
- `Mutex<T>`: Mutual exclusion
- `RwLock<T>`: Reader-writer lock

### Async
```rust
#[tokio::main]
async fn main() {
    let handle = tokio::spawn(async {
        // async work
    });
    handle.await.unwrap();
}
```

## Skill Files

For detailed guidance, see:
- [Question routing](https://github.com/actionbook/rust-skills/blob/main/skills/rust-router/SKILL.md)
- [Code style rules](https://github.com/actionbook/rust-skills/blob/main/skills/coding-guidelines/SKILL.md)
- [Unsafe code review](https://github.com/actionbook/rust-skills/blob/main/skills/unsafe-checker/SKILL.md)
- [Ownership concepts](https://github.com/actionbook/rust-skills/blob/main/skills/m01-ownership/SKILL.md)
- [Error patterns](https://github.com/actionbook/rust-skills/blob/main/skills/m06-error-handling/SKILL.md)
- [Concurrency patterns](https://github.com/actionbook/rust-skills/blob/main/skills/m07-concurrency/SKILL.md)
