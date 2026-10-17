#!/bin/sh
# Smoke test of the Flatpak bundle, run on a CI Linux runner:
#   1. install the bundle (the KDE runtime is already installed by the build);
#   2. start the app inside the sandbox with `--diagnostics-script scroll-zoom` on a fixture, on a
#      virtual X display: exit code 0, which needs the engine to start and render inside the sandbox;
#   3. run the engine sandbox probe inside the sandbox (seccomp filters stack there, ADR-0018) and
#      require what crates/engine/tests/sandbox_linux.rs requires on the host.
#
#   packaging/flatpak/smoke.sh <bundle.flatpak> <fixture.pdf> <engine_sandbox_probe>
set -eu
bundle=$1
fixture=$2
probe=$3
app=io.github.jhjh1214.Vellora

work=$(mktemp -d)
trap 'rm -rf "$work"' EXIT
cp "$fixture" "$work/fixture.pdf"
cp "$probe" "$work/engine_sandbox_probe"
chmod +x "$work/engine_sandbox_probe"

flatpak --user install -y --noninteractive --bundle "$bundle"

echo "== diagnostics script inside the sandbox"
xvfb-run -a dbus-run-session -- \
  flatpak run --filesystem="$work:ro" "$app" --diagnostics-script scroll-zoom "$work/fixture.pdf" \
  | tee "$work/script.txt"
grep -q 'diagnostics-script scroll-zoom:' "$work/script.txt"

echo "== engine sandbox probe inside the sandbox"
flatpak run --filesystem="$work:ro" --command="$work/engine_sandbox_probe" "$app" | tee "$work/probe.txt"
grep -q 'landlock: Ok(Full)' "$work/probe.txt" || { echo "::error::Landlock is not enforced in full inside the Flatpak"; exit 1; }
grep -q 'seccomp: Ok(())' "$work/probe.txt" || { echo "::error::seccomp did not install inside the Flatpak"; exit 1; }
for name in "read the library directory" "read its own /proc entry" "start a thread"; do
  grep -q "^$name: before=ok after=ok" "$work/probe.txt" || { echo "::error::'$name' must keep working"; exit 1; }
done
for name in "read a file outside the allowed set" "create a file" "read the parent's /proc entry" "a socket" "start a program"; do
  grep -Eq "^$name: before=ok after=err\\((1|13)\\)" "$work/probe.txt" || { echo "::error::'$name' must work before and be refused after"; exit 1; }
done
echo "flatpak smoke test passed"
