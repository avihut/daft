import { readdirSync, readFileSync, statSync } from "node:fs";
import { join } from "node:path";
import matter from "gray-matter";

export type Recipe = {
  title: string;
  description: string;
  link: string;
  pillars: string[];
  tooling: string[];
  languages: string[];
};

export function loadRecipes(recipesDir: string): Recipe[] {
  const recipes: Recipe[] = [];
  for (const category of ["by-tooling", "by-language", "by-scenario"]) {
    const dir = join(recipesDir, category);
    if (!statSync(dir, { throwIfNoEntry: false })?.isDirectory()) continue;
    for (const file of readdirSync(dir)) {
      if (!file.endsWith(".md")) continue;
      const path = join(dir, file);
      const raw = readFileSync(path, "utf8");
      const { data } = matter(raw);
      recipes.push({
        title: data.title ?? file.replace(/\.md$/, ""),
        description: data.description ?? "",
        link: `/recipes/${category}/${file.replace(/\.md$/, "")}`,
        pillars: Array.isArray(data.pillars) ? data.pillars : [],
        tooling: Array.isArray(data.tooling) ? data.tooling : [],
        languages: Array.isArray(data.languages) ? data.languages : [],
      });
    }
  }
  return recipes;
}
