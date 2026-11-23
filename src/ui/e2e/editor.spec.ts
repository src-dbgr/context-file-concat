import { test, expect, type Page } from "@playwright/test";

/**
 * The editor in the real build:
 * - Monaco is not part of the initial load, it comes with the first preview;
 * - syntax highlighting works with the base editor worker only (no language
 *   worker is requested, nothing fails to load);
 * - quick previews end up in one editor;
 * - search matches are highlighted once and follow the content search.
 */

type Preview = { content: string; language: string; path: string };

/** A file per language the host names (src/utils/language.rs), plus Svelte. */
const SAMPLES: Preview[] = [
  {
    language: "typescript",
    path: "/repo/a.ts",
    content: "export const x: number = 1; // note\n",
  },
  {
    language: "javascript",
    path: "/repo/a.js",
    content: "function f() { return 'a'; } // note\n",
  },
  {
    language: "json",
    path: "/repo/a.json",
    content: '{ "a": 1, "b": [true, null], "c": "x" }\n',
  },
  {
    language: "rust",
    path: "/repo/a.rs",
    content: 'fn main() { let x = "a"; } // note\n',
  },
  {
    language: "python",
    path: "/repo/a.py",
    content: "def f():\n    return 'a'  # note\n",
  },
  {
    language: "html",
    path: "/repo/a.html",
    content: '<div class="a">x</div>\n',
  },
  {
    language: "css",
    path: "/repo/a.css",
    content: ".a { color: red; } /* note */\n",
  },
  {
    language: "markdown",
    path: "/repo/a.md",
    content: "# Title\n\nSome **bold** text.\n",
  },
  { language: "yaml", path: "/repo/a.yaml", content: 'a: 1\nb: "x" # note\n' },
  {
    language: "shell",
    path: "/repo/a.sh",
    content: 'echo "hi" # note\nif true; then exit 0; fi\n',
  },
  {
    language: "go",
    path: "/repo/a.go",
    content: 'package main\nfunc main() { x := "a" } // note\n',
  },
  {
    language: "java",
    path: "/repo/A.java",
    content: 'class A { String s = "a"; } // note\n',
  },
  {
    language: "c",
    path: "/repo/a.c",
    content: 'int main(void) { return 0; } /* note */ char *s = "a";\n',
  },
  {
    language: "cpp",
    path: "/repo/a.cpp",
    content: 'int main() { return 0; } // note\nconst char *s = "a";\n',
  },
  {
    language: "plaintext",
    path: "/repo/App.svelte",
    content: '<script>let a = 1;</script>\n<p class="a">x</p>\n',
  },
];

test.beforeEach(async ({ page }) => {
  // There is no host in the preview; swallow what the UI posts to it.
  await page.addInitScript(() => {
    (window as unknown as { ipc: { postMessage: (m: string) => void } }).ipc = {
      postMessage: () => {},
    };
  });
});

async function openApp(page: Page) {
  await page.goto("/?e2e=1");
  await page.waitForFunction(() => {
    const w = window as unknown as {
      __e2e?: { store?: { setAppState?: (s: unknown) => void } };
    };
    return typeof w.__e2e?.store?.setAppState === "function";
  });
}

function showPreview(page: Page, preview: Preview, searchTerm = "") {
  return page.evaluate(
    ([p, term]) => {
      (
        window as unknown as {
          showPreviewContent: (
            c: string,
            l: string,
            s: string | null,
            p: string,
          ) => void;
        }
      ).showPreviewContent(p.content, p.language, term, p.path);
    },
    [preview, searchTerm] as const,
  );
}

/** Number of different token colors in the editor: > 1 means highlighted. */
function tokenColors(page: Page) {
  return page.evaluate(() => {
    const classes = new Set<string>();
    for (const span of document.querySelectorAll(
      "#editor-container .view-lines span[class*='mtk']",
    )) {
      for (const c of span.classList) if (c.startsWith("mtk")) classes.add(c);
    }
    return classes.size;
  });
}

/** Changes the content search of the app state, as a snapshot of the host does. */
function setContentSearch(page: Page, query: string) {
  return page.evaluate((q) => {
    const w = window as unknown as {
      __e2e: {
        store: { setAppState: (s: unknown) => void };
        debug: { dump: () => { app: Record<string, unknown> } };
      };
    };
    const app = w.__e2e.debug.dump().app;
    w.__e2e.store.setAppState({ ...app, content_search_query: q });
  }, query);
}

test("Monaco is loaded with the first preview, not with the app", async ({
  page,
}) => {
  const requested: string[] = [];
  page.on("request", (r) => requested.push(r.url()));

  await openApp(page);
  await expect(page.locator("#generate-btn")).toBeVisible();

  const html = await (await page.request.get("/")).text();
  expect(html).not.toMatch(/monaco/i);
  expect(requested.filter((u) => /monaco|editor\.worker/i.test(u))).toEqual([]);

  await showPreview(page, SAMPLES[0]);
  await expect(page.locator("#editor-container .monaco-editor")).toHaveCount(1);
  expect(requested.some((u) => /monacoBundle\.js/.test(u))).toBe(true);
});

test("Highlights every language with the base editor worker only", async ({
  page,
}) => {
  const failures: string[] = [];
  page.on("requestfailed", (r) => failures.push(`failed: ${r.url()}`));
  page.on("response", (r) => {
    if (r.status() >= 400) failures.push(`${r.status()}: ${r.url()}`);
  });
  page.on("pageerror", (e) => failures.push(`pageerror: ${e.message}`));
  page.on("console", (m) => {
    if (m.type() === "error") failures.push(`console: ${m.text()}`);
  });

  await openApp(page);

  for (const sample of SAMPLES) {
    await showPreview(page, sample);
    await expect(page.locator("#preview-title")).toContainText(
      sample.path.split("/").pop()!,
    );
    await expect
      .poll(() => tokenColors(page), {
        message: `token colors for ${sample.path}`,
      })
      .toBeGreaterThan(1);
  }

  // Plain text stays plain.
  await showPreview(page, {
    language: "plaintext",
    path: "/repo/notes.txt",
    content: "just some text\n",
  });
  await expect(page.locator("#preview-title")).toContainText("notes.txt");
  await expect.poll(() => tokenColors(page)).toBe(1);

  // Only the base editor worker is ever started.
  const workers = page.workers().map((w) => w.url());
  expect(workers.filter((u) => !/editor\.worker/.test(u))).toEqual([]);
  expect(failures).toEqual([]);
});

test("Two quick previews share one editor", async ({ page }) => {
  await openApp(page);

  // Both arrive before Monaco has loaded.
  await page.evaluate(
    ([a, b]) => {
      const show = (
        window as unknown as {
          showPreviewContent: (
            c: string,
            l: string,
            s: string | null,
            p: string,
          ) => void;
        }
      ).showPreviewContent;
      show(a.content, a.language, "", a.path);
      show(b.content, b.language, "", b.path);
    },
    [SAMPLES[0], SAMPLES[3]] as const,
  );

  await expect(page.locator("#preview-title")).toContainText("a.rs");
  await expect(page.locator("#editor-container .monaco-editor")).toHaveCount(1);
  await expect(page.locator("#editor-container .view-lines")).toContainText(
    "fn main()",
  );
});

test("Search matches are highlighted once and follow the content search", async ({
  page,
}) => {
  await openApp(page);
  const highlights = page.locator("#editor-container .search-highlight");

  const file: Preview = {
    language: "plaintext",
    path: "/repo/notes.txt",
    content: "alpha beta alpha\ngamma alpha beta\n",
  };

  await showPreview(page, file, "alpha");
  await expect(highlights).toHaveCount(3);

  // The same preview again must not stack a second set of decorations.
  await showPreview(page, file, "alpha");
  await expect(highlights).toHaveCount(3);

  await setContentSearch(page, "beta");
  await expect(highlights).toHaveCount(2);

  await setContentSearch(page, "");
  await expect(highlights).toHaveCount(0);

  // Generated output is not searched.
  await setContentSearch(page, "alpha");
  await expect(highlights).toHaveCount(3);
  await page.evaluate(() => {
    (
      window as unknown as {
        showGeneratedContent: (c: string, t: number) => void;
      }
    ).showGeneratedContent("alpha alpha\n", 2);
  });
  await expect(page.locator("#preview-title")).toContainText(
    /Preview generated/i,
  );
  await expect(highlights).toHaveCount(0);
});
