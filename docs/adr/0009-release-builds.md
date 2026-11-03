# 0009. Release builds are native, old on Linux and scanned before publishing

Status: accepted

## Decision
Each target builds on its own runner of that platform, because BoringSSL is built with CMake and does not
cross-compile well. Linux builds on an old image, so the binary links against an old glibc and runs
on old distributions. macOS and Windows build on the newest explicit image, since their binaries do not inherit
the runner's version. Windows ARM64 builds with clang-cl and Ninja, because the Visual Studio generator hands
BoringSSL's assembly to `cl.exe`, which cannot assemble it.

Release binaries are built with cargo-auditable, so their crate list can be scanned. The release job makes a
CycloneDX SBOM, scans the binaries and the SBOM, and refuses to publish on a known vulnerability. Only the
publish job holds write access, and the build jobs use no cache. A manual run of the workflow does everything
except publish. CI runs the tests on the newest explicit images, one per OS and architecture. The macOS tests are
a separate workflow and not required checks, because GitHub's free macOS runners are often unavailable and a
job that never starts would otherwise turn the main CI badge red.

## Forbidden
- A build cache in the job that builds the shipped binaries.
- Write access for a job that runs build scripts.
- Publishing before the scan passes.
- The moving `-latest` runner labels in the test matrix and the release matrix.

## Revisit when
GitHub's macOS runners become dependable, which would let those checks be required, or when `btls-sys`
builds on Windows ARM64 without clang-cl.

## Cost if wrong
A release fails at tag time instead of in review, or a binary needs a newer glibc than a user's system has.
