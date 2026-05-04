import { dirname, resolve } from "node:path";
import { fileURLToPath } from "node:url";
import { defineLoader } from "vitepress";
import { loadRecipes } from "../theme/recipe-list";

const __dirname = dirname(fileURLToPath(import.meta.url));

export interface Data {
  title: string;
  description: string;
  link: string;
  pillars: string[];
  tooling: string[];
  languages: string[];
}

declare const data: Data[];

export { data };

export default defineLoader({
  watch: ["../../recipes/**/*.md"],
  load() {
    const recipesDir = resolve(__dirname, "../../recipes");
    return loadRecipes(recipesDir);
  },
});
