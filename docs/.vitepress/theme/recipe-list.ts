import { readdirSync, readFileSync, statSync } from "node:fs";
import { join } from "node:path";
import matter from "gray-matter";

export type RecipeKind =
  | "pattern"
  | "walkthrough"
  | "anti-pattern"
  | "reference";

export type Recipe = {
  title: string;
  description: string;
  link: string;
  kind: RecipeKind;
  pillars: string[];
};

const REFERENCE_FILES = new Set(["sharing-caches.md", "troubleshooting.md"]);

function readMarkdownFiles(
  dir: string,
): { file: string; data: matter.GrayMatterFile<string>["data"] }[] {
  if (!statSync(dir, { throwIfNoEntry: false })?.isDirectory()) return [];
  return readdirSync(dir)
    .filter((f) => f.endsWith(".md"))
    .map((file) => {
      const raw = readFileSync(join(dir, file), "utf8");
      const { data } = matter(raw);
      return { file, data };
    });
}

function toRecipe(
  file: string,
  data: matter.GrayMatterFile<string>["data"],
  link: string,
  kind: RecipeKind,
): Recipe {
  return {
    title: data.title ?? file.replace(/\.md$/, ""),
    description: data.description ?? "",
    link,
    kind,
    pillars: Array.isArray(data.pillars) ? data.pillars : [],
  };
}

export function loadRecipes(recipesDir: string): Recipe[] {
  const recipes: Recipe[] = [];

  // Top-level: patterns + reference pages (sharing-caches.md). Skip index.md.
  for (const { file, data } of readMarkdownFiles(recipesDir)) {
    if (file === "index.md") continue;
    const slug = file.replace(/\.md$/, "");
    const kind: RecipeKind = REFERENCE_FILES.has(file)
      ? "reference"
      : "pattern";
    recipes.push(toRecipe(file, data, `/recipes/${slug}`, kind));
  }

  // walkthroughs/
  for (const { file, data } of readMarkdownFiles(
    join(recipesDir, "walkthroughs"),
  )) {
    const slug = file.replace(/\.md$/, "");
    recipes.push(
      toRecipe(file, data, `/recipes/walkthroughs/${slug}`, "walkthrough"),
    );
  }

  // anti-patterns/
  for (const { file, data } of readMarkdownFiles(
    join(recipesDir, "anti-patterns"),
  )) {
    const slug = file.replace(/\.md$/, "");
    recipes.push(
      toRecipe(file, data, `/recipes/anti-patterns/${slug}`, "anti-pattern"),
    );
  }

  return recipes;
}
