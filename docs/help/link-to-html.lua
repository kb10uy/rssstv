-- Links between pages are written as `.md` so the sources stay browsable where
-- Markdown is rendered, and point at the built pages once they are.
function Link(link)
  link.target = link.target:gsub("^([%w%-]+)%.md(#?[^#]*)$", "%1.html%2")
  return link
end
