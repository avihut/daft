# daft Rebranding Brief

## Current Direction

The selected brand direction for **daft** is a minimalist dodo-based logo mark named **Donut**.

The logo represents daft as a friendly but mature developer tool: playful enough to be memorable, simple enough to work as a CLI/project icon, and structured enough to feel credible in a docs site, GitHub repo, package listing, and terminal-oriented workflow.

## Product Context

**daft** is a Git extensions toolkit focused on Git worktree workflows.

Core idea:

> Stop switching branches. Give every branch its own clean working nest.

The product helps developers manage multiple isolated worktrees so they can work on several branches, hotfixes, reviews, and experiments simultaneously without constantly stashing, switching branches, reinstalling dependencies, or losing IDE/build context.

## Brand Metaphor

### Primary Mascot: Dodo

The dodo gives the project a memorable mascot identity.

It works because:

- **daft** already has a playful name.
- A dodo is distinctive, charming, and slightly silly without being generic.
- The dodo can become a recognizable symbol for the tool even when the name is not shown.
- It creates room for a friendly open-source personality without making the product feel unserious.

### Companion Metaphor: Nest

The nest is the secondary metaphor.

In daft, each worktree is like a managed nest: a clean, isolated place where a branch can live with its own dependencies, build artifacts, IDE state, and context.

Potential tagline ideas:

> Stop switching branches. Give every branch a nest.

> A careful little dodo for your Git worktree nests.

> One repo. Many nests. No context switching.

## Selected Logo Candidate

### Name

**Donut**

### Description

Donut is an abstract dodo mark built from bold black-and-white shapes. The form combines:

- a simplified dodo head
- a stronger hooked beak
- a small circular eye
- a flowing neck/body curve
- a lowercase **d** created through negative space
- an open loop shape that keeps the mark from feeling too heavy
- a vertical stem that reinforces the lowercase **d** identity

### Why Donut Works

Donut is the current best logo candidate because it has the strongest balance of personality, maturity, and functionality.

It passes the most important logo criteria:

- recognizable as a bird/dodo-inspired mark
- memorable due to the hidden negative-space **d**
- usable without the wordmark
- works in black and white
- strong enough for GitHub avatars, docs navbars, package pages, terminal contexts, and stickers
- simple enough to be redrawn from memory
- playful without becoming too childish

### Important Design Principle

The mark should feel like:

> a smart, minimal dodo that happens to contain a lowercase **d**

Not:

> a clever letterform that vaguely looks like a bird

## Rejected / Secondary Candidate

### Name

**Doodle**

Doodle was another strong candidate. It was friendlier and more child-drawable, but less mature and less ownable than Donut.

Doodle should remain a useful emotional reference: warm, simple, and charming. Future refinements of Donut should preserve some of Doodle’s softness while keeping Donut’s stronger structure and negative-space **d**.

## Logo Design Guidelines

The logo should remain:

- **black and white at the core**
- simple
- abstract
- readable at small sizes
- recognizable from silhouette
- free of gradients, shadows, textures, and decorative detail
- compatible with both light and dark backgrounds
- suitable for SVG implementation

Avoid:

- detailed feathers
- feet
- nest objects inside the logo
- literal Git branch diagrams
- multiple colors in the primary mark
- complex mascot illustration
- gradients or 3D shading
- tiny details that vanish at 16px

## Tests the Logo Should Pass

### 1. Black on White

The canonical mark should work as solid black on a white background.

Use case:

- README header
- docs site
- printed material
- SVG icon
- monochrome package listings

### 2. White on Black

The mark should invert cleanly to white on black.

Use case:

- dark mode docs
- terminal screenshots
- CLI splash/help contexts
- GitHub social cards

### 3. Small Size Test

Test the mark at:

- 64px
- 32px
- 24px
- 16px
- 12px

At very small sizes, the eye and beak may become fragile. If needed, create a small-size optimized variant with:

- slightly larger eye
- more open beak cut
- simplified inner curve
- slightly increased spacing around the negative-space **d**

### 4. Container Test

Test the mark inside:

- circle
- outlined circle
- filled circle
- square
- rounded square
- app-icon-style rounded rectangle

Watch for:

- beak getting too close to the edge
- vertical stem making the mark feel too tall
- right side feeling too heavy
- insufficient breathing room

### 5. Negative-Space Circle Test

Try:

- black mark on white circle
- white mark knocked out of black circle
- black mark in white ring
- white mark inside dark rounded square

This is especially important for GitHub avatars and social icons.

### 6. Silhouette Test

Remove the internal eye and negative-space **d** and inspect only the outer shape.

Question:

> Does it still feel like a dodo/bird-like mark, or does it become a generic blob?

Donut depends strongly on its negative space, which is okay, but the outer contour should still feel intentional.

### 7. One-Color Print Test

The logo should survive:

- stamp
- sticker
- embroidery
- laser engraving
- low-resolution print
- package-manager icon rendering

Avoid hairline strokes or internal details that require high resolution.

### 8. Favicon / Browser Tab Test

Check whether the logo remains recognizable in:

- browser tab favicon
- docs site sidebar
- mobile home screen icon
- GitHub repo avatar

A separate favicon-optimized version may be useful.

### 9. Wordmark Pairing Test

Test the mark beside the lowercase wordmark:

```text
dafft
```

Correction: the actual wordmark is:

```text
daft
```

Recommended pairings:

- icon left, wordmark right
- icon above, wordmark below
- compact lockup for docs navbar
- standalone icon for favicon/avatar

The preferred feel is:

> playful mark, mature wordmark

### 10. Clear Space Test

Keep clear space around the logo.

Suggested rule:

> Clear space should be at least the diameter of the eye on all sides.

For cramped icon contexts, use an optical adjustment rather than scaling the mark to touch the edges.

## Color Direction

The primary logo should remain black and white.

Color should be used as an accent in docs, UI highlights, buttons, stickers, favicons, and secondary brand assets.

### Recommended Core Palette

```txt
Primary black:   #111111
Background:      #FFFFFF
Soft background: #F6F3EE
Primary accent:  #D99A21  Dodo Beak Gold
Dark accent:     #C75C1E  Rust Orange
Secondary:       #1B9AAA  Tropical Teal
```

## Primary Accent

### Dodo Beak Gold

```txt
#D99A21
```

This is the recommended main accent color.

Why it works:

- connects directly to the dodo beak
- adds warmth and personality
- works well with black and white
- feels playful but not childish
- can be used sparingly in docs and UI

Recommended uses:

- hover states
- callout borders
- selected nav item
- docs accents
- small beak detail in special mascot variant
- release badges
- social card highlights

Avoid using it to replace the primary black mark in normal contexts.

## Secondary Accent Options

### Rust Orange

```txt
#C75C1E
```

Good for:

- Rust/devtool association
- stronger technical feel
- warnings or emphasis
- darker warm accents

### Nest Clay

```txt
#B86B45
```

Good for:

- warmer docs illustrations
- nest metaphor
- background accents
- companion brand elements

### Tropical Teal

```txt
#1B9AAA
```

Good for:

- contrast against warm colors
- links or secondary actions
- diagrams
- modern technical accent

Use sparingly so the brand does not drift away from the dodo/nest concept.

## Logo Variants to Prepare

### 1. Primary Mark

Black Donut on transparent background.

Use for:

- docs header
- README
- package listings
- print
- general branding

### 2. Reverse Mark

White Donut on transparent background.

Use for:

- dark mode
- terminal-themed contexts
- dark social cards

### 3. Icon Container Variant

White Donut on black circle or rounded square.

Use for:

- GitHub avatar
- favicon fallback
- app-icon-style usage
- social preview avatar

### 4. Accent Variant

Black Donut with Dodo Beak Gold accent, only if it still works simply.

Use for:

- website hero
- stickers
- launch announcement
- docs landing page

This should be a secondary expressive variant, not the canonical logo.

### 5. Tiny/Favicon Variant

A simplified version optimized for 16–32px.

Potential adjustments:

- larger eye
- slightly larger beak opening
- less fragile internal curve
- more visual space around the negative-space **d**

## Suggested File Structure

```txt
assets/
  brand/
    logo/
      daft-donut.svg
      daft-donut-black.svg
      daft-donut-white.svg
      daft-donut-circle.svg
      daft-donut-rounded-square.svg
      daft-donut-accent.svg
      daft-donut-favicon.svg
    color/
      palette.md
    social/
      og-image-template.svg
      github-social-preview.png
```

Or, for a docs site:

```txt
public/
  brand/
    daft-logo.svg
    daft-logo-dark.svg
    daft-icon.svg
    daft-icon-circle.svg
    favicon.svg
    apple-touch-icon.png
    og-image.png
```

## README Usage

Suggested README header treatment:

```md
<p align="center">
  <img src="./public/brand/daft-logo.svg" alt="daft" width="120" />
</p>

<h1 align="center">daft</h1>

<p align="center">
  Stop switching branches. Give every branch a nest.
</p>
```

Alternative simpler header:

```md
# daft

> Stop switching branches. Give every branch a nest.
```

With the logo placed in the docs site rather than the README title area.

## Docs Site Tone

The docs site should feel:

- friendly
- fast
- focused
- developer-native
- slightly playful
- not too polished/SaaS-like
- not childish

Suggested visual tone:

- black and white foundation
- Dodo Beak Gold accents
- lots of whitespace
- simple diagrams of repo/worktree structure
- occasional dodo/nest language in callouts
- avoid heavy mascot illustration except in special sections

## Messaging Ideas

Primary:

> Stop switching branches. Give every branch a nest.

More direct:

> Work on multiple Git branches at once, without losing context.

CLI/devtool:

> Git worktrees without the workflow friction.

Mascot-flavored:

> A careful little dodo for your Git worktree nests.

Hero section option:

> One repo. Many branches. Each in its own nest.

## Brand Do / Don’t

### Do

- Use Donut as the primary mark.
- Keep the core mark black and white.
- Use Dodo Beak Gold as an accent.
- Preserve the negative-space **d**.
- Keep the beak slightly hooked and dodo-like.
- Use nest language sparingly but intentionally.
- Test the logo at small sizes before finalizing.

### Don’t

- Add feathers, feet, shadows, gradients, or detailed scenery.
- Turn the logo into a full Apple-emoji-style dodo illustration.
- Use too many colors in the mark.
- Make the beak too duck-like or parrot-like.
- Let the vertical stem feel glued on.
- Overuse the mascot language in serious technical docs.

## Open Refinement Notes

The selected Donut candidate is strong, but before finalizing as SVG, check:

- Is the eye large enough at 16px?
- Does the beak read as dodo, not duck?
- Does the negative-space **d** remain clear at small sizes?
- Is the right vertical stem visually integrated?
- Does the logo feel centered in a square/circle crop?
- Does the mark still feel mature next to the `daft` wordmark?

## Final Recommendation

Proceed with **Donut** as the primary logo direction.

Use the current black-on-white Donut mark as the basis for a clean SVG redraw. Keep the structure and negative-space **d**, but refine the geometry manually rather than relying on generated-image output.

The final brand system should be:

- **Primary logo:** black-and-white Donut
- **Mascot concept:** dodo
- **Companion metaphor:** nest
- **Primary accent:** Dodo Beak Gold `#D99A21`
- **Tone:** mature devtool with a playful open-source mascot edge

