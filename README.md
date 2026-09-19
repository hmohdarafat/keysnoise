# keysnoise
Keyboard noises

# 1. Build tools + libraries
sudo apt update
sudo apt install -y build-essential pkg-config libgtk-4-dev libasound2-dev
curl --proto '=https' --tlsv1.2 -sSf https://sh.rustup.rs | sh   # then restart the terminal

# 2. Sounds: copy your own keyboard noise wav folder into assets
cp -r /path/to/original/key/noises/wav ./assets/wav

# 3. Allow reading keyboard events (needed on Wayland and X11), then log out and back in
sudo usermod -aG input $USER

# 4. Build and run
cargo run --release
The app will run and you might run into some permission issues. If that happened to you then keep reading....
Do this instead if you want to start right away and you won't have to logging out and logging back in OR restart your computer:
sg input -c "cargo run --release"