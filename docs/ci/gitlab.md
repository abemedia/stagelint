---
description: Check merge requests and pushes with stagelint in GitLab CI/CD.
---

# GitLab CI/CD

Add a job to `.gitlab-ci.yml`:

```yaml
stagelint:
  rules:
    - if: $CI_PIPELINE_SOURCE == "merge_request_event"
    - if: $CI_COMMIT_BRANCH == $CI_DEFAULT_BRANCH
  variables:
    # Fetch the full history so the base commit is available.
    GIT_DEPTH: 0
  script:
    # Install the tools your stagelint config runs.

    # Install and run stagelint.
    - curl -fsSL https://stagelint.dev/install.sh | sh
    - |
      base=${CI_MERGE_REQUEST_DIFF_BASE_SHA:-$CI_COMMIT_BEFORE_SHA}
      if git cat-file -e "$base^{commit}" 2>/dev/null; then
        stagelint --diff "$base...HEAD"
      else
        stagelint --all
      fi

    # Fail the job when a command changed a file.
    - git diff --exit-code
```

The job runs stagelint on the files the merge request or push changed, and fails when a command
fails or changes a file.

## Files checked

| Pipeline                                      | Files                                                             |
| --------------------------------------------- | ----------------------------------------------------------------- |
| Merge request                                 | Changed since the merge request's branch diverged from its target |
| Push to the default branch                    | Changed since the previous commit on the branch                   |
| Scheduled or manual run on the default branch | Every file                                                        |

Merged results pipelines and merge trains also check the files the target branch changed since the
merge request's branch diverged. If the base commit is not in the fetched history, such as after a
force push, every file is checked.
