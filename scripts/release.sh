#!/bin/bash
# Release automation script for pinner.
# Bumps version, runs checks, runs tests, updates lockfile, commits, and tags the release.

set -e

# Colors for output
GREEN='\033[0;32m'
YELLOW='\033[1;33m'
RED='\033[0;31m'
NC='\033[0m' # No Color

echo -e "${GREEN}==> Starting release preparation...${NC}"

# 1. Ensure git is available
if ! command -v git &> /dev/null; then
    echo -e "${RED}Error: git is not installed or not in PATH.${NC}"
    exit 1
fi

# 2. Check for uncommitted changes
if ! git diff-index --quiet HEAD --; then
    echo -e "${RED}Error: Working directory has uncommitted changes. Please commit or stash them first.${NC}"
    git status -s
    exit 1
fi

# 3. Check current branch (detect detached HEAD and warn if not main)
CURRENT_BRANCH=$(git branch --show-current)
if [ -z "$CURRENT_BRANCH" ]; then
    echo -e "${RED}Error: You are in a detached HEAD state. Please checkout a branch (typically 'main') before releasing.${NC}"
    exit 1
fi

if [ "$CURRENT_BRANCH" != "main" ]; then
    echo -e "${YELLOW}Warning: You are not on the 'main' branch (current: $CURRENT_BRANCH).${NC}"
    read -p "Do you want to proceed with the release on this branch anyway? (y/N) " confirm
    if [[ ! "$confirm" =~ ^[yY]$ ]]; then
        echo "Release aborted."
        exit 1
    fi
fi

# 4. Verify remote alignment and synchronization
REMOTE="origin"
REMOTE_EXISTS=false
if git remote get-url "$REMOTE" &> /dev/null; then
    REMOTE_EXISTS=true
fi

TRACKING_REF=""
INITIAL_REMOTE_REV=""

if [ "$REMOTE_EXISTS" = true ]; then
    echo -e "${GREEN}==> Fetching latest changes and tags from remote '$REMOTE'...${NC}"
    if ! git fetch "$REMOTE" "$CURRENT_BRANCH" --tags --prune; then
        echo -e "${YELLOW}Warning: Failed to fetch from remote '$REMOTE' (network offline or permission denied).${NC}"
        read -p "Do you want to bypass remote verification and proceed anyway? (y/N) " bypass_remote
        if [[ ! "$bypass_remote" =~ ^[yY]$ ]]; then
            echo "Release aborted."
            exit 1
        fi
    else
        # Determine upstream tracking ref or remote branch
        if git rev-parse --verify --quiet "@{u}" &> /dev/null; then
            TRACKING_REF="@{u}"
        elif git rev-parse --verify --quiet "$REMOTE/$CURRENT_BRANCH" &> /dev/null; then
            TRACKING_REF="$REMOTE/$CURRENT_BRANCH"
        fi

        if [ -n "$TRACKING_REF" ]; then
            LOCAL_REV=$(git rev-parse HEAD)
            INITIAL_REMOTE_REV=$(git rev-parse "$TRACKING_REF")
            BASE_REV=$(git merge-base HEAD "$TRACKING_REF")

            if [ "$LOCAL_REV" = "$INITIAL_REMOTE_REV" ]; then
                echo -e "Branch '${GREEN}$CURRENT_BRANCH${NC}' is up to date with '$TRACKING_REF'."
            elif [ "$LOCAL_REV" = "$BASE_REV" ]; then
                BEHIND_COUNT=$(git rev-list --count HEAD.."$TRACKING_REF")
                echo -e "${RED}Error: Local branch '$CURRENT_BRANCH' is behind '$TRACKING_REF' by $BEHIND_COUNT commit(s).${NC}"
                echo -e "${RED}Release cannot proceed because pushing would be rejected.${NC}"
                echo -e "Please synchronize your local branch first:"
                echo -e "  ${YELLOW}git pull --rebase $REMOTE $CURRENT_BRANCH${NC}"
                exit 1
            elif [ "$INITIAL_REMOTE_REV" = "$BASE_REV" ]; then
                AHEAD_COUNT=$(git rev-list --count "$TRACKING_REF"..HEAD)
                echo -e "${YELLOW}Notice: Local branch '$CURRENT_BRANCH' is ahead of '$TRACKING_REF' by $AHEAD_COUNT unpushed commit(s).${NC}"
                read -p "Do you want to proceed with unpushed commits included in this release? (y/N) " ahead_confirm
                if [[ ! "$ahead_confirm" =~ ^[yY]$ ]]; then
                    echo "Release aborted."
                    exit 1
                fi
            else
                BEHIND_COUNT=$(git rev-list --count HEAD.."$TRACKING_REF")
                AHEAD_COUNT=$(git rev-list --count "$TRACKING_REF"..HEAD)
                echo -e "${RED}Error: Local branch '$CURRENT_BRANCH' and '$TRACKING_REF' have diverged!${NC}"
                echo -e "${RED}Local is ahead by $AHEAD_COUNT commit(s) and behind by $BEHIND_COUNT commit(s).${NC}"
                echo -e "Please reconcile the branches before releasing:"
                echo -e "  ${YELLOW}git pull --rebase $REMOTE $CURRENT_BRANCH${NC}"
                exit 1
            fi
        else
            echo -e "${YELLOW}Notice: No remote tracking branch found for '$CURRENT_BRANCH' on '$REMOTE'. A new branch will be pushed.${NC}"
        fi
    fi
else
    echo -e "${YELLOW}Notice: Remote '$REMOTE' not configured. Skipping remote synchronization check.${NC}"
fi

# 5. Extract current version from Cargo.toml
CARGO_TOML="Cargo.toml"
if [ ! -f "$CARGO_TOML" ]; then
    echo -e "${RED}Error: $CARGO_TOML not found in the current directory.${NC}"
    exit 1
fi

CURRENT_VERSION=$(grep -m1 '^version = ' "$CARGO_TOML" | cut -d '"' -f2)
if [ -z "$CURRENT_VERSION" ]; then
    echo -e "${RED}Error: Could not extract version from $CARGO_TOML.${NC}"
    exit 1
fi

echo -e "Current Cargo.toml version: ${GREEN}$CURRENT_VERSION${NC}"

# Parse version components for bumping
IFS='.' read -r major minor patch <<< "$CURRENT_VERSION"
# Clean patch if it has metadata/pre-release suffix
patch=$(echo "$patch" | cut -d '-' -f1 | cut -d '+' -f1)

SUGGESTED_PATCH="$major.$minor.$((patch + 1))"
SUGGESTED_MINOR="$major.$((minor + 1)).0"
SUGGESTED_MAJOR="$((major + 1)).0.0"

# Determine target version
TARGET_VERSION=""
if [ -n "$1" ]; then
    case "$1" in
        patch)  TARGET_VERSION="$SUGGESTED_PATCH" ;;
        minor)  TARGET_VERSION="$SUGGESTED_MINOR" ;;
        major)  TARGET_VERSION="$SUGGESTED_MAJOR" ;;
        *)      TARGET_VERSION="$1" ;;
    esac
else
    echo "Select next version bump type:"
    echo "1) Patch ($CURRENT_VERSION -> $SUGGESTED_PATCH)"
    echo "2) Minor ($CURRENT_VERSION -> $SUGGESTED_MINOR)"
    echo "3) Major ($CURRENT_VERSION -> $SUGGESTED_MAJOR)"
    echo "4) Custom version string"
    read -p "Select option (1-4): " choice
    case "$choice" in
        1) TARGET_VERSION="$SUGGESTED_PATCH" ;;
        2) TARGET_VERSION="$SUGGESTED_MINOR" ;;
        3) TARGET_VERSION="$SUGGESTED_MAJOR" ;;
        4) 
            read -p "Enter custom version: " TARGET_VERSION
            ;;
        *)
            echo -e "${RED}Invalid selection. Aborting.${NC}"
            exit 1
            ;;
    esac
fi

# Validate target version format
if [[ ! "$TARGET_VERSION" =~ ^[0-9]+\.[0-9]+\.[0-9]+(-[a-zA-Z0-9.]+)?$ ]]; then
    echo -e "${RED}Error: Target version '$TARGET_VERSION' is not a valid semantic version.${NC}"
    exit 1
fi

# Ensure tag doesn't already exist locally
if git rev-parse "refs/tags/v$TARGET_VERSION" >/dev/null 2>&1; then
    echo -e "${RED}Error: Git tag v$TARGET_VERSION already exists locally.${NC}"
    exit 1
fi

# Ensure tag doesn't already exist on remote
if [ "$REMOTE_EXISTS" = true ]; then
    if git ls-remote --tags "$REMOTE" "refs/tags/v$TARGET_VERSION" 2>/dev/null | grep -q "refs/tags/v$TARGET_VERSION"; then
        echo -e "${RED}Error: Git tag v$TARGET_VERSION already exists on remote '$REMOTE'.${NC}"
        exit 1
    fi
fi

echo -e "Target version: ${GREEN}$TARGET_VERSION${NC}"
read -p "Proceed with releasing v$TARGET_VERSION? (y/N) " confirm
if [[ ! "$confirm" =~ ^[yY]$ ]]; then
    echo "Release aborted."
    exit 1
fi

revert_working_tree() {
    git checkout "$CARGO_TOML" 2>/dev/null || true
    if git ls-files --error-unmatch Cargo.lock >/dev/null 2>&1; then
        git checkout Cargo.lock 2>/dev/null || true
    fi
    if [ -f "$CHANGELOG" ] && git ls-files --error-unmatch "$CHANGELOG" >/dev/null 2>&1; then
        git checkout "$CHANGELOG" 2>/dev/null || true
    fi
}

# 6. Update version in Cargo.toml
echo -e "${GREEN}==> Updating version in Cargo.toml...${NC}"
sed "s/^version = \"$CURRENT_VERSION\"/version = \"$TARGET_VERSION\"/" "$CARGO_TOML" > "$CARGO_TOML.tmp" && mv "$CARGO_TOML.tmp" "$CARGO_TOML"

# 7. Update Cargo.lock by running cargo check
echo -e "${GREEN}==> Updating Cargo.lock and verifying Cargo configuration...${NC}"
if ! cargo check; then
    echo -e "${RED}Error: cargo check failed. Reverting Cargo.toml.${NC}"
    revert_working_tree
    exit 1
fi

# 8. Check CHANGELOG.md
CHANGELOG="CHANGELOG.md"
if [ -f "$CHANGELOG" ]; then
    if ! grep -q "\[$TARGET_VERSION\]" "$CHANGELOG"; then
        echo -e "${YELLOW}Warning: $CHANGELOG does not seem to contain an entry for [$TARGET_VERSION].${NC}"
        echo -e "Please update the changelog to document changes for version $TARGET_VERSION."
        read -p "Press Enter to continue checking after you update $CHANGELOG, or Ctrl+C to abort..."
        if ! grep -q "\[$TARGET_VERSION\]" "$CHANGELOG"; then
            echo -e "${RED}Error: $CHANGELOG still does not contain [$TARGET_VERSION]. Reverting changes.${NC}"
            revert_working_tree
            exit 1
        fi
    fi
else
    echo -e "${YELLOW}Warning: CHANGELOG.md not found. Skipping changelog check.${NC}"
fi

# 9. Run unit and integration tests
echo -e "${GREEN}==> Running tests to verify release safety...${NC}"
if ! cargo test; then
    echo -e "${RED}Error: Tests failed. Reverting changes.${NC}"
    revert_working_tree
    exit 1
fi

# 10. Verify Cargo.toml version corresponds to TARGET_VERSION right before tagging
FINAL_CARGO_VERSION=$(grep -m1 '^version = ' "$CARGO_TOML" | cut -d '"' -f2)
if [ "$FINAL_CARGO_VERSION" != "$TARGET_VERSION" ]; then
    echo -e "${RED}Error: Critical mismatch! Cargo.toml version ($FINAL_CARGO_VERSION) does not match target release version ($TARGET_VERSION).${NC}"
    revert_working_tree
    exit 1
fi

# 11. Mid-flight race check: Ensure remote did not advance while tests were running
if [ "$REMOTE_EXISTS" = true ] && [ -n "$TRACKING_REF" ] && [ -n "$INITIAL_REMOTE_REV" ]; then
    echo -e "${GREEN}==> Verifying remote alignment has not changed during test execution...${NC}"
    if git fetch "$REMOTE" "$CURRENT_BRANCH" --quiet 2>/dev/null; then
        LATEST_REMOTE_REV=$(git rev-parse "$TRACKING_REF")
        if [ "$LATEST_REMOTE_REV" != "$INITIAL_REMOTE_REV" ]; then
            echo -e "${RED}Error: Remote branch '$TRACKING_REF' advanced while tests were running!${NC}"
            echo -e "${RED}Aborting release commit to prevent push rejection.${NC}"
            revert_working_tree
            echo -e "Please synchronize with the remote branch before releasing:"
            echo -e "  ${YELLOW}git pull --rebase $REMOTE $CURRENT_BRANCH${NC}"
            exit 1
        fi
    fi
fi

# 12. Commit changes
echo -e "${GREEN}==> Committing changes...${NC}"
git add "$CARGO_TOML"
if [ -f "Cargo.lock" ]; then
    git add Cargo.lock
fi
if [ -f "$CHANGELOG" ]; then
    git add "$CHANGELOG"
fi
git commit -m "chore: bump version to $TARGET_VERSION"

# 13. Tag the release
echo -e "${GREEN}==> Creating git tag v$TARGET_VERSION...${NC}"
git tag -a "v$TARGET_VERSION" -m "Release v$TARGET_VERSION"

echo -e "${GREEN}==============================================${NC}"
echo -e "${GREEN}Release v$TARGET_VERSION successfully prepared locally!${NC}"
echo -e "${GREEN}==============================================${NC}"
echo -e "A git commit and git tag (v$TARGET_VERSION) have been created."
echo ""
echo -e "To push the commit and tag to the remote repository atomically, run:"
echo -e "  ${YELLOW}git push --atomic origin $CURRENT_BRANCH refs/tags/v$TARGET_VERSION${NC}"
echo ""

read -p "Would you like to push the changes now? (y/N) " push_confirm
if [[ "$push_confirm" =~ ^[yY]$ ]]; then
    echo -e "${GREEN}==> Pushing commit and tag atomically to $REMOTE...${NC}"
    if git push --atomic "$REMOTE" "$CURRENT_BRANCH" "refs/tags/v$TARGET_VERSION"; then
        echo -e "${GREEN}==============================================${NC}"
        echo -e "${GREEN}Release v$TARGET_VERSION successfully published to $REMOTE!${NC}"
        echo -e "${GREEN}==============================================${NC}"
    else
        echo -e "${RED}==============================================${NC}"
        echo -e "${RED}Error: Atomic push failed!${NC}"
        echo -e "${RED}Because --atomic was used, NEITHER the branch NOR the tag was updated on remote.${NC}"
        echo -e "${RED}This prevented an orphaned tag on the remote.${NC}"
        echo -e "${RED}==============================================${NC}"
        echo -e "To investigate and retry manually, run:"
        echo -e "  ${YELLOW}git push --atomic $REMOTE $CURRENT_BRANCH refs/tags/v$TARGET_VERSION${NC}"
        echo ""
        echo -e "To rollback the local release commit and tag:"
        echo -e "  ${YELLOW}git tag -d v$TARGET_VERSION${NC}"
        echo -e "  ${YELLOW}git reset --hard HEAD~1${NC}"
        exit 1
    fi
else
    echo "To push later, use:"
    echo -e "  ${YELLOW}git push --atomic $REMOTE $CURRENT_BRANCH refs/tags/v$TARGET_VERSION${NC}"
fi
