-- .testing.lua -- what the test sheets of this repository run against (testing.nvim, docs/SHEETS.md there). The sheets
-- in TESTS/sheets only name an app; how it starts is here, under testing.nvim's trust gate. pult has no Lua spec.

return {
	sheets = {
		-- the Chrome of the machine, like tools/page-check.mjs; playwright-core comes at a fixed version from that folder
		browser = { playwright_dir = "tools/page-check", channel = "chrome" },
		apps = {
			-- The hub with a demo mixer: no audio device, nothing of the user's settings (a folder of the run).
			["demo-hub"] = {
				build = { "cargo", "build", "-p", "pult-hub", "--example", "demo-hub", "--locked" },
				run = { "target/debug/examples/demo-hub.exe" },
				env = { PULT_PORT = "0", PULT_CONFIG_DIR = "${temp}" },
				ready = { stdout = "^http", as = "link" },
			},
		},
	},
}
