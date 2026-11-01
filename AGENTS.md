# Agent instructions

## Bumping the version

Use the Justfile, and never edit version numbers by hand:

```sh
just bump patch   # 0.1.1 -> 0.1.2
just bump minor   # 0.1.1 -> 0.2.0
```

This updates the crates, the VS Code extension and its README together, then creates the `Release <version>` commit and the
`v<version>` tag. The working tree must be clean first.

It doesn't push. Pushing the tag publishes a release, so only do that when asked to.
