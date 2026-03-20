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

-- Editor defaults
vim.opt.termguicolors = true
vim.opt.number = true
vim.opt.wrap = true
vim.opt.linebreak = true
vim.opt.conceallevel = 2

-- Re-apply number after plugins load (some plugins override it)
vim.api.nvim_create_autocmd("FileType", {
  pattern = "markdown",
  callback = function()
    vim.opt_local.number = true
  end,
})

-- Toggle markdown checkbox on current line with <CR> in normal mode
vim.keymap.set("n", "<Space>", function()
  local line = vim.api.nvim_get_current_line()
  local new_line = line:gsub("%- %[( )%]", "- [x]", 1)
  if new_line == line then
    new_line = line:gsub("%- %[x%]", "- [ ]", 1)
  end
  if new_line ~= line then
    vim.api.nvim_set_current_line(new_line)
    vim.cmd("silent write")
  end
end, { desc = "Toggle checkbox" })
