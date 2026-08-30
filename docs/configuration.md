# Router configuration

The configuration is YAML by default. A file ending in `.json` is parsed as
JSON. Unknown fields are rejected, including token-like fields.

Profiles identify accounts; they do not contain credentials. Routes are
evaluated in this order:

```text
exact repository > repository glob > owner > host > default
```

At one specificity, different profiles produce an ambiguous result. A write
must not fall back to `default_profile` unless
`ambiguity_policy.write: default_profile` is explicitly configured. The safer
default is to fail closed.

## Personal and work accounts

This is a complete two-account configuration using synthetic usernames and an
organization name. Replace the identity references with usernames shown by
`gh auth status`.

```yaml
profiles:
  personal:
    provider: gh
    user: personal-user
    host: github.com

  work:
    provider: gh
    user: work-user
    host: github.com

routes:
  - match:
      owner: PersonalUser
    profile: personal

  - match:
      owner: ExampleOrg
    profile: work

default_profile: personal

ambiguity_policy:
  read: default_profile
  write: error
```

Check the result without resolving a credential:

```bash
gh-mcp-router route ExampleOrg/backend --config config.yaml
gh-mcp-router explain ExampleOrg/backend --config config.yaml
```

## Personal account plus organization route

An owner route is enough when all repositories in an organization use the same
identity:

```yaml
profiles:
  personal:
    provider: gh
    user: personal-user
  work:
    provider: gh
    user: work-user

routes:
  - match: { owner: ExampleOrg }
    profile: work
  - match: { owner: personal-user }
    profile: personal

default_profile: personal
ambiguity_policy: { read: default_profile, write: error }
```

## Repository-specific override

More specific rules win over an owner rule. This sends one repository back to
the personal profile while other `ExampleOrg` repositories use `work`:

```yaml
profiles:
  personal:
    provider: gh
    user: personal-user
  work:
    provider: gh
    user: work-user

routes:
  - match:
      repo: ExampleOrg/public-docs
    profile: personal
  - match:
      repo: ExampleOrg/security-*
    profile: work
  - match:
      owner: ExampleOrg
    profile: work

default_profile: personal
ambiguity_policy: { read: default_profile, write: error }
```

`repo` is `OWNER/REPOSITORY`; a single `*` is supported for a repository glob.
Do not use a route to rewrite the owner, repository, or host supplied by an MCP
call.

## GitHub Enterprise or another custom host

Custom GitHub hosts are supported as host values without a scheme or path:

```yaml
profiles:
  enterprise:
    provider: gh
    user: enterprise-user
    host: github.example.com
    gh_config_dir: ${HOME}/.config/gh-enterprise

routes:
  - match:
      host: github.example.com
    profile: enterprise

ambiguity_policy:
  read: error
  write: error
```

Authenticate that host with the same `gh` host value and ensure the referenced
config directory exists. The router passes the host to the selected upstream
child; it does not silently map a custom host to `github.com`.

## No-default, fail-closed configuration

Omit `default_profile` and make both policies explicit when every request must
carry enough repository context to select a profile:

```yaml
profiles:
  personal: { provider: gh, user: personal-user }
  work: { provider: gh, user: work-user }

routes:
  - match: { owner: PersonalUser }
    profile: personal
  - match: { owner: ExampleOrg }
    profile: work

ambiguity_policy:
  read: error
  write: error
```

With this policy, `route`/`explain` report a no-match or ambiguous decision,
and an MCP write without safe context is rejected rather than guessed.

## Profile-specific `GH_CONFIG_DIR`

Use `gh_config_dir` when two profiles should not share GitHub CLI state:

```yaml
profiles:
  personal:
    provider: gh
    user: personal-user
    gh_config_dir: ${HOME}/.config/gh-personal
  work:
    provider: gh
    user: work-user
    gh_config_dir: ${HOME}/.config/gh-work
```

The router expands `${HOME}` and `~` at the credential boundary. It never
persists the resolved credential in this file.
