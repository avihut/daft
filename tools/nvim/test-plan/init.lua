-- Minimal neovim config for markdown test plans with rendered checkboxes.
-- Launched via the sandbox `test-plan` command with isolated XDG paths
-- so nothing touches the user's personal neovim setup.

-- Bootstrap lazy.nvim
local lazypath = vim.fn.stdpath("data") .. "/lazy/lazy.nvim"
if not (vim.uv or vim.loop).fs_stat(lazypath) then
  local out = vim.fn.system({
    "git", "clone", "--filter=blob:none", "--branch=stable",
    "https://github.com/folke/lazy.nvim.git", lazypath,
  })
  if vim.v.shell_error ~= 0 then
    vim.api.nvim_echo({
      { "Failed to clone lazy.nvim:\n", "ErrorMsg" },
      { out, "WarningMsg" },
    }, true, {})
    os.exit(1)
  end
end
vim.opt.rtp:prepend(lazypath)

-- Plugins
require("lazy").setup({
  {
    "nvim-treesitter/nvim-treesitter",
    build = ":TSUpdate",
    opts = {
      ensure_installed = { "markdown", "markdown_inline" },
      highlight = { enable = true },
    },
  },
  {
    "MeanderingProgrammer/render-markdown.nvim",
    dependencies = { "nvim-treesitter/nvim-treesitter" },
    ft = { "markdown" },
    opts = {},
  },
}, {
  install = { colorscheme = { "habamax" } },
  checker = { enabled = false },
  change_detection = { enabled = false },
})

-- Markdown defaults
vim.opt.termguicolors = true
vim.opt.number = true
vim.opt.wrap = true
vim.opt.linebreak = true
vim.opt.conceallevel = 2
