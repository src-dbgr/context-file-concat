#!/bin/bash

set -e

# Colors for output
RED='\033[0;31m'
GREEN='\033[0;32m'
YELLOW='\033[1;33m'
NC='\033[0m' # No Color

# Function to print colored output
print_status() {
    echo -e "${GREEN}[INFO]${NC} $1"
}

print_warning() {
    echo -e "${YELLOW}[WARN]${NC} $1"
}

print_error() {
    echo -e "${RED}[ERROR]${NC} $1"
}

# Check if version argument is provided
if [ $# -eq 0 ]; then
    print_error "Please provide a version number"
    echo "Usage: $0 <version> [--dry-run]"
    echo "Example: $0 0.1.0"
    echo "         $0 0.1.0 --dry-run"
    exit 1
fi

VERSION=$1
DRY_RUN=false

if [ "$2" = "--dry-run" ]; then
    DRY_RUN=true
    print_warning "Running in dry-run mode - no changes will be made"
fi

# Validate version format (semantic versioning)
if ! [[ $VERSION =~ ^[0-9]+\.[0-9]+\.[0-9]+(-[a-zA-Z0-9]+)?$ ]]; then
    print_error "Invalid version format. Use semantic versioning (e.g., 1.0.0, 1.0.0-beta)"
    exit 1
fi

# Check if we're on main branch
CURRENT_BRANCH=$(git branch --show-current)
if [ "$CURRENT_BRANCH" != "main" ]; then
    print_warning "Not on main branch (current: $CURRENT_BRANCH)"
    read -p "Continue anyway? (y/N): " -n 1 -r
    echo
    if [[ ! $REPLY =~ ^[Yy]$ ]]; then
        exit 1
    fi
fi

# Check for uncommitted changes
if ! git diff-index --quiet HEAD --; then
    print_error "You have uncommitted changes. Please commit or stash them first."
    exit 1
fi

print_status "Preparing release v$VERSION"

# Update version in Cargo.toml
if [ "$DRY_RUN" = false ]; then
    print_status "Updating Cargo.toml version to $VERSION"
    sed -i.bak "s/^version = \".*\"/version = \"$VERSION\"/" Cargo.toml
    rm Cargo.toml.bak
fi

# Run tests to ensure everything works
print_status "Running tests..."
if [ "$DRY_RUN" = false ]; then
    cargo test
else
    print_warning "Skipping tests (dry-run)"
fi

# Build frontend
print_status "Building frontend..."
if [ "$DRY_RUN" = false ]; then
    cd src/ui
    npm install
    npm run build
    cd ../..
else
    print_warning "Skipping frontend build (dry-run)"
fi

# Check that binary builds successfully
print_status "Testing release build..."
if [ "$DRY_RUN" = false ]; then
    cargo build --release
else
    print_warning "Skipping release build test (dry-run)"
fi

# Generate changelog entry (if CHANGELOG.md exists)
if [ -f "CHANGELOG.md" ]; then
    print_status "Please update CHANGELOG.md with changes for v$VERSION"
    if [ "$DRY_RUN" = false ]; then
        read -p "Press Enter after updating CHANGELOG.md..."
    fi
fi

# Commit version changes
if [ "$DRY_RUN" = false ]; then
    print_status "Committing version bump"
    git add Cargo.toml
    if [ -f "CHANGELOG.md" ]; then
        git add CHANGELOG.md
    fi
    git commit -m "Bump version to v$VERSION"
fi

# Create and push tag
if [ "$DRY_RUN" = false ]; then
    print_status "Creating and pushing tag v$VERSION"
    git tag -a "v$VERSION" -m "Release v$VERSION"
    git push origin main
    git push origin "v$VERSION"
    
    print_status "🎉 Release v$VERSION initiated!"
    print_status "GitHub Actions will now build and publish the release."
    print_status "Check the progress at: https://github.com/$(git config --get remote.origin.url | sed 's/.*github.com[:/]\([^.]*\).*/\1/')/actions"
else
    print_warning "Would create tag v$VERSION and push to GitHub (dry-run)"
fi

print_status "Release process completed!"

if [ "$DRY_RUN" = false ]; then
    echo
    echo "Next steps:"
    echo "1. Monitor the GitHub Actions build"
    echo "2. Test the generated binaries"
    echo "3. Update release notes on GitHub if needed"
    echo "4. Announce the release"
fi