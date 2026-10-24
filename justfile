# List available recipes
[private]
default:
	@just --list

# Test the vault and the window helpers
test *args:
	cargo test {{args}}

# Open the desktop window
app *args:
	cargo run -p runebook-app {{args}}
