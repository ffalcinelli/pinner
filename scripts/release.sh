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

# 3. Check current branch (warn if not main)
CURRENT_BRANCH=$(git branch --show-current)
if [ "$CURRENT_BRANCH" != "main" ]; then
    echo -e "${YELLOW}Warning: You are not on the 'main' branch (current: $CURRENT_BRANCH).${NC}"
    read -p "Do you want to proceed with the release on this branch anyway? (y/N) " confirm
    if [[ ! "$confirm" =~ ^[yY]$ ]]; then
        echo "Release aborted."
        exit 1
    fi
fi

# 4. Extract current version from Cargo.toml
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

# Ensure tag doesn't already exist
if git rev-parse "v$TARGET_VERSION" >/dev/null 2>&1; then
    echo -e "${RED}Error: Git tag v$TARGET_VERSION already exists.${NC}"
    exit 1
fi

echo -e "Target version: ${GREEN}$TARGET_VERSION${NC}"
read -p "Proceed with releasing v$TARGET_VERSION? (y/N) " confirm
if [[ ! "$confirm" =~ ^[yY]$ ]]; then
    echo "Release aborted."
    exit 1
fi

# 5. Update version in Cargo.toml
echo -e "${GREEN}==> Updating version in Cargo.toml...${NC}"
sed "s/^version = \"$CURRENT_VERSION\"/version = \"$TARGET_VERSION\"/" "$CARGO_TOML" > "$CARGO_TOML.tmp" && mv "$CARGO_TOML.tmp" "$CARGO_TOML"

# 6. Update Cargo.lock by running cargo check
echo -e "${GREEN}==> Updating Cargo.lock and verifying Cargo configuration...${NC}"
if ! cargo check; then
    echo -e "${RED}Error: cargo check failed. Reverting Cargo.toml.${NC}"
    git checkout "$CARGO_TOML"
    exit 1
fi

# 7. Check CHANGELOG.md
CHANGELOG="CHANGELOG.md"
if [ -f "$CHANGELOG" ]; then
    if ! grep -q "\[$TARGET_VERSION\]" "$CHANGELOG"; then
        echo -e "${YELLOW}Warning: $CHANGELOG does not seem to contain an entry for [$TARGET_VERSION].${NC}"
        echo -e "Please update the changelog to document changes for version $TARGET_VERSION."
        read -p "Press Enter to continue checking after you update $CHANGELOG, or Ctrl+C to abort..."
        if ! grep -q "\[$TARGET_VERSION\]" "$CHANGELOG"; then
            echo -e "${RED}Error: $CHANGELOG still does not contain [$TARGET_VERSION]. Reverting changes.${NC}"
            git checkout "$CARGO_TOML" Cargo.lock
            exit 1
        fi
    fi
else
    echo -e "${YELLOW}Warning: CHANGELOG.md not found. Skipping changelog check.${NC}"
fi

# 8. Run unit and integration tests
echo -e "${GREEN}==> Running tests to verify release safety...${NC}"
if ! cargo test; then
    echo -e "${RED}Error: Tests failed. Reverting changes.${NC}"
    git checkout "$CARGO_TOML" Cargo.lock
    if [ -f "$CHANGELOG" ]; then
        git checkout "$CHANGELOG"
    fi
    exit 1
fi

# 9. Verify Cargo.toml version corresponds to TARGET_VERSION right before tagging
FINAL_CARGO_VERSION=$(grep -m1 '^version = ' "$CARGO_TOML" | cut -d '"' -f2)
if [ "$FINAL_CARGO_VERSION" != "$TARGET_VERSION" ]; then
    echo -e "${RED}Error: Critical mismatch! Cargo.toml version ($FINAL_CARGO_VERSION) does not match target release version ($TARGET_VERSION).${NC}"
    git checkout "$CARGO_TOML" Cargo.lock
    if [ -f "$CHANGELOG" ]; then
        git checkout "$CHANGELOG"
    fi
    exit 1
fi

# 10. Commit changes
echo -e "${GREEN}==> Committing changes...${NC}"
git add "$CARGO_TOML" Cargo.lock
if [ -f "$CHANGELOG" ]; then
    git add "$CHANGELOG"
fi
git commit -m "chore: bump version to $TARGET_VERSION"

# 11. Tag the release
echo -e "${GREEN}==> Creating git tag v$TARGET_VERSION...${NC}"
git tag -a "v$TARGET_VERSION" -m "Release v$TARGET_VERSION"

echo -e "${GREEN}==============================================${NC}"
echo -e "${GREEN}Release v$TARGET_VERSION successfully prepared locally!${NC}"
echo -e "${GREEN}==============================================${NC}"
echo -e "A git commit and git tag (v$TARGET_VERSION) have been created."
echo ""
echo -e "To push the commit and tag to the remote repository, run:"
echo -e "  ${YELLOW}git push origin $CURRENT_BRANCH --tags${NC}"
echo ""

read -p "Would you like to push the changes now? (y/N) " push_confirm
if [[ "$push_confirm" =~ ^[yY]$ ]]; then
    git push origin "$CURRENT_BRANCH" --tags
else
    echo "To push later, use: git push origin $CURRENT_BRANCH --tags"
fi
