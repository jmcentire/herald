use axum::response::Html;

pub async fn landing_page() -> Html<&'static str> {
    Html(LANDING_HTML)
}

const LANDING_HTML: &str = r##"<!DOCTYPE html>
<html lang="en">
<head>
<meta charset="utf-8">
<meta name="viewport" content="width=device-width, initial-scale=1">
<title>Herald — Webhook relay for AI agents</title>
<meta name="description" content="Your agent doesn't need to be always-on. Herald is. Lightweight webhook relay and message queue for AI agents and local services.">
<meta name="theme-color" content="#0a0a14">
<link rel="preconnect" href="https://fonts.googleapis.com">
<link rel="preconnect" href="https://fonts.gstatic.com" crossorigin>
<link href="https://fonts.googleapis.com/css2?family=Inter:wght@400;500;600;700&family=JetBrains+Mono:wght@400;500&display=swap" rel="stylesheet">
<style>
*,*::before,*::after{box-sizing:border-box;margin:0;padding:0}
:root{
  --bg:#0a0a14;--surface:#0f0f1a;--card:#161626;--card-hover:#1c1c32;
  --border:#2a2a3e;--border-bright:#3a3a52;
  --fg:#e8e6e3;--secondary:#9d9b97;--muted:#6b6966;--dim:#3d3b50;
  --accent:#22d3ee;--accent-dim:#0e7490;
  --signet:#c9a227;--kindex:#3b82f6;
  --font-sans:"Inter",system-ui,sans-serif;
  --font-mono:"JetBrains Mono",ui-monospace,monospace;
}
html{background:var(--bg);color:var(--fg);font-family:var(--font-sans);line-height:1.6;-webkit-font-smoothing:antialiased}
a{color:var(--secondary);text-decoration:none;transition:color .15s}
a:hover{color:var(--fg)}
.accent{color:var(--accent)}
.mono{font-family:var(--font-mono)}

.max-w{max-width:64rem;margin:0 auto;padding:0 1.5rem}
.border-t{border-top:1px solid var(--border)}

/* Header */
header{border-bottom:1px solid var(--border);padding:1rem 0}
header .inner{display:flex;align-items:center;justify-content:space-between}
header .brand{font-family:var(--font-mono);font-weight:700;font-size:1rem;color:var(--fg);display:flex;align-items:center;gap:.5rem}
header .brand .dot{width:8px;height:8px;border-radius:50%;background:var(--accent);display:inline-block}
header nav{display:flex;gap:1.5rem}
header nav a{font-size:.875rem;color:var(--muted)}
header nav a:hover{color:var(--fg)}

/* Hero */
.hero{padding:5rem 0 4rem;text-align:center}
.hero h1{font-size:2.5rem;font-weight:700;line-height:1.2;margin-bottom:1rem;letter-spacing:-.02em}
.hero .tagline{font-size:1.125rem;color:var(--secondary);max-width:36rem;margin:0 auto 2rem}
.hero .cta{display:inline-flex;align-items:center;gap:.5rem;background:var(--accent-dim);color:var(--accent);font-family:var(--font-mono);font-size:.875rem;padding:.75rem 1.5rem;border-radius:.5rem;border:1px solid var(--accent);transition:background .15s}
.hero .cta:hover{background:var(--accent);color:var(--bg)}

/* Terminal */
.terminal{background:var(--surface);border:1px solid var(--border);border-radius:.75rem;padding:1.5rem;max-width:40rem;margin:2rem auto 0;text-align:left;font-family:var(--font-mono);font-size:.8rem;line-height:1.8;color:var(--secondary);overflow-x:auto}
.terminal .prompt{color:var(--muted)}
.terminal .cmd{color:var(--fg)}
.terminal .out{color:var(--accent)}
.terminal .comment{color:var(--dim)}

/* How it works */
.how{padding:4rem 0}
.how h2{font-size:1.5rem;font-weight:700;margin-bottom:2rem;text-align:center}
.flow{display:flex;flex-direction:column;gap:1rem;max-width:40rem;margin:0 auto}
.flow .step{display:flex;gap:1rem;align-items:flex-start}
.flow .num{font-family:var(--font-mono);font-size:.75rem;color:var(--accent);background:var(--card);border:1px solid var(--border);border-radius:.375rem;padding:.25rem .5rem;min-width:2rem;text-align:center;flex-shrink:0}
.flow .desc{font-size:.9375rem;color:var(--secondary)}
.flow .desc strong{color:var(--fg);font-weight:600}

/* Features grid */
.features{padding:4rem 0}
.features h2{font-size:1.5rem;font-weight:700;margin-bottom:2rem;text-align:center}
.grid{display:grid;grid-template-columns:repeat(auto-fit,minmax(16rem,1fr));gap:1rem}
.card{background:var(--card);border:1px solid var(--border);border-radius:.75rem;padding:1.5rem;transition:border-color .15s}
.card:hover{border-color:var(--border-bright)}
.card h3{font-size:1rem;font-weight:600;margin-bottom:.5rem;display:flex;align-items:center;gap:.5rem}
.card h3 .icon{font-size:1.125rem}
.card p{font-size:.875rem;color:var(--secondary);line-height:1.6}

/* Tiers */
.tiers{padding:4rem 0}
.tiers h2{font-size:1.5rem;font-weight:700;margin-bottom:.5rem;text-align:center}
.tiers .sub{text-align:center;color:var(--secondary);font-size:.9375rem;margin-bottom:2rem}
.tier-grid{display:grid;grid-template-columns:repeat(auto-fit,minmax(14rem,1fr));gap:1rem}
.tier{background:var(--card);border:1px solid var(--border);border-radius:.75rem;padding:1.5rem;position:relative}
.tier.active{border-color:var(--accent)}
.tier .badge{position:absolute;top:-.625rem;left:1.5rem;background:var(--accent);color:var(--bg);font-family:var(--font-mono);font-size:.6875rem;font-weight:600;padding:.125rem .625rem;border-radius:1rem}
.tier h3{font-family:var(--font-mono);font-size:1.125rem;font-weight:700;margin-bottom:.25rem}
.tier .price{font-size:.875rem;color:var(--muted);margin-bottom:1rem}
.tier ul{list-style:none;font-size:.8125rem;color:var(--secondary)}
.tier ul li{padding:.25rem 0;padding-left:1rem;position:relative}
.tier ul li::before{content:">";position:absolute;left:0;color:var(--accent);font-family:var(--font-mono)}

/* Security */
.security{padding:4rem 0}
.security h2{font-size:1.5rem;font-weight:700;margin-bottom:2rem;text-align:center}

/* CLI */
.cli{padding:4rem 0}
.cli h2{font-size:1.5rem;font-weight:700;margin-bottom:.5rem;text-align:center}
.cli .sub{text-align:center;color:var(--secondary);font-size:.9375rem;margin-bottom:2rem}

/* Ecosystem */
.ecosystem{padding:4rem 0}
.ecosystem h2{font-size:1.5rem;font-weight:700;margin-bottom:2rem;text-align:center}
.eco-grid{display:grid;grid-template-columns:repeat(auto-fit,minmax(14rem,1fr));gap:1rem;max-width:40rem;margin:0 auto}
.eco-card{background:var(--card);border:1px solid var(--border);border-radius:.75rem;padding:1.25rem;text-align:center}
.eco-card .name{font-weight:600;font-size:.9375rem;margin-bottom:.25rem}
.eco-card .role{font-size:.8125rem;color:var(--muted)}

/* Footer */
footer{border-top:1px solid var(--border);padding:3rem 0}
footer .cols{display:grid;grid-template-columns:repeat(4,1fr);gap:2rem;margin-bottom:2rem}
footer h4{font-family:var(--font-mono);font-size:.6875rem;color:var(--muted);text-transform:uppercase;letter-spacing:.1em;margin-bottom:1rem}
footer ul{list-style:none}
footer ul li{padding:.25rem 0}
footer ul li a{font-size:.8125rem;color:var(--secondary)}
footer ul li a:hover{color:var(--fg)}
footer .current{color:var(--accent);font-weight:500}
footer .signet:hover{color:var(--signet)}
footer .kindex:hover{color:var(--kindex)}
footer .bottom{border-top:1px solid var(--border);padding-top:1.5rem;display:flex;justify-content:space-between;flex-wrap:wrap;gap:1rem}
footer .bottom span{font-family:var(--font-mono);font-size:.6875rem;color:var(--dim)}

@media(max-width:640px){
  .hero h1{font-size:1.75rem}
  footer .cols{grid-template-columns:repeat(2,1fr)}
  .tier-grid{grid-template-columns:1fr}
}
</style>
</head>
<body>

<header>
<div class="max-w inner">
  <a href="/" class="brand"><span class="dot"></span> Herald</a>
  <nav>
    <a href="#how">How it works</a>
    <a href="#features">Features</a>
    <a href="#tiers">Pricing</a>
    <a href="https://github.com/jmcentire/herald">GitHub</a>
  </nav>
</div>
</header>

<section class="hero max-w">
  <h1>Your agent doesn't need to be always-on.<br><span class="accent">Herald is.</span></h1>
  <p class="tagline">Lightweight webhook relay and message queue for AI agents and local services that can't expose a public endpoint. Give your agent a stable URL. Herald receives, queues, and delivers.</p>
  <a href="https://github.com/jmcentire/herald" class="cta">View on GitHub</a>

  <div class="terminal">
    <div><span class="comment"># Point any webhook at your Herald URL</span></div>
    <div><span class="prompt">$ </span><span class="cmd">curl -X POST https://proxy.herald.tools/myagent/github \</span></div>
    <div><span class="cmd">    -d '{"action":"push","ref":"refs/heads/main"}'</span></div>
    <div><span class="out">{"message_id":"422609...","fingerprint":"e3d03e..."}</span></div>
    <div>&nbsp;</div>
    <div><span class="comment"># Your agent polls when it's ready</span></div>
    <div><span class="prompt">$ </span><span class="cmd">curl -H "Authorization: Bearer $KEY" \</span></div>
    <div><span class="cmd">    https://proxy.herald.tools/queue/github</span></div>
    <div><span class="out">{"messages":[{"message_id":"422609...","body":"..."}]}</span></div>
    <div>&nbsp;</div>
    <div><span class="comment"># ACK and move on</span></div>
    <div><span class="prompt">$ </span><span class="cmd">curl -X POST -H "Authorization: Bearer $KEY" \</span></div>
    <div><span class="cmd">    https://proxy.herald.tools/ack/github/422609...</span></div>
    <div><span class="out">{"acknowledged":true}</span></div>
  </div>
</section>

<section id="how" class="how max-w">
  <h2>How it works</h2>
  <div class="flow">
    <div class="step"><span class="num">1</span><div class="desc"><strong>Point webhooks at Herald.</strong> Every account gets a stable inbound URL. GitHub, Stripe, your orchestration layer — anything that sends HTTP POST.</div></div>
    <div class="step"><span class="num">2</span><div class="desc"><strong>Herald queues them.</strong> Payloads are hashed for deduplication, encrypted on receipt, and held in a FIFO queue. No plaintext at rest.</div></div>
    <div class="step"><span class="num">3</span><div class="desc"><strong>Your agent drains the queue.</strong> HTTP polling for simple agents, WebSocket for low-latency event-driven workflows. Connect when ready.</div></div>
    <div class="step"><span class="num">4</span><div class="desc"><strong>ACK and move on.</strong> Acknowledge processed messages. Failed? They go back in the queue or to a dead letter queue after retries.</div></div>
  </div>
</section>

<section id="features" class="features max-w">
  <h2>Built for agents</h2>
  <div class="grid">
    <div class="card"><h3>Inbound routing</h3><p>Named endpoints that map to separate queues. One URL for GitHub, another for Stripe, another for your orchestration layer.</p></div>
    <div class="card"><h3>Encrypted on receipt</h3><p>AES-256-GCM encryption before storage. Bring your own key on Pro — Herald never sees your plaintext.</p></div>
    <div class="card"><h3>Content-addressable</h3><p>SHA-256 fingerprinting deduplicates identical payloads at ingestion. Replay protection built in.</p></div>
    <div class="card"><h3>Poll or stream</h3><p>HTTP polling for simple agents. WebSocket with first-message auth for low-latency event-driven workflows.</p></div>
    <div class="card"><h3>At-least-once delivery</h3><p>Visibility timeouts, automatic redelivery, dead letter queues. Messages don't get lost.</p></div>
    <div class="card"><h3>Self-hostable</h3><p>MIT licensed. Single Rust binary + Redis. Run your own Herald anywhere. No vendor lock-in.</p></div>
  </div>
</section>

<section class="cli max-w">
  <h2>herald-cli</h2>
  <p class="sub">Optional local daemon. Polls Herald, invokes your agent, reports ACK/NACK.</p>
  <div class="terminal">
    <div><span class="comment"># ~/.config/herald/config.yaml</span></div>
    <div><span class="cmd">server: https://proxy.herald.tools</span></div>
    <div><span class="cmd">api_key: hrl_sk_...</span></div>
    <div><span class="cmd">connection: websocket</span></div>
    <div>&nbsp;</div>
    <div><span class="cmd">handlers:</span></div>
    <div><span class="cmd">  github-push:</span></div>
    <div><span class="cmd">    command: claude</span></div>
    <div><span class="cmd">    args: ["-p"]</span></div>
    <div><span class="cmd">    prompt_template: |</span></div>
    <div><span class="cmd">      Use kindex to search for context.</span></div>
    <div><span class="cmd">      Then handle this event: </span><span class="out">{{.body}}</span></div>
    <div><span class="cmd">    stdin: prompt</span></div>
    <div><span class="cmd">    hooks:</span></div>
    <div><span class="cmd">      pre:</span></div>
    <div><span class="cmd">        command: kindex</span></div>
    <div><span class="cmd">        args: ["ingest", "--tags", "herald", "--stdin"]</span></div>
  </div>
</section>

<section id="tiers" class="tiers max-w">
  <h2>Pricing</h2>
  <p class="sub">Free tier available now. Paid tiers coming soon.</p>
  <div class="tier-grid">
    <div class="tier active">
      <span class="badge">Available now</span>
      <h3>Free</h3>
      <div class="price">$0/month</div>
      <ul>
        <li>1 inbound endpoint</li>
        <li>HTTP polling</li>
        <li>100 messages/day</li>
        <li>7-day retention</li>
        <li>Encrypted at rest</li>
      </ul>
    </div>
    <div class="tier">
      <h3>Standard</h3>
      <div class="price">$12/month — coming soon</div>
      <ul>
        <li>10 endpoints</li>
        <li>Polling + WebSocket</li>
        <li>10,000 messages/day</li>
        <li>30-day retention</li>
        <li>Headers included</li>
      </ul>
    </div>
    <div class="tier">
      <h3>Pro</h3>
      <div class="price">$49/month — coming soon</div>
      <ul>
        <li>Unlimited endpoints</li>
        <li>500,000 messages/day</li>
        <li>90-day retention</li>
        <li>BYOK encryption</li>
        <li>Audit log</li>
      </ul>
    </div>
    <div class="tier">
      <h3>Enterprise</h3>
      <div class="price">Custom</div>
      <ul>
        <li>Custom volume</li>
        <li>Custom retention</li>
        <li>Custom SLAs</li>
        <li>Private deployment</li>
      </ul>
    </div>
  </div>
</section>

<section class="security max-w">
  <h2>Security</h2>
  <div class="grid">
    <div class="card"><h3>TLS everywhere</h3><p>All inbound and outbound connections over TLS. No plaintext HTTP.</p></div>
    <div class="card"><h3>Encrypt on receipt</h3><p>AES-256-GCM with per-account keys. Plaintext exists only briefly in RAM during ingestion.</p></div>
    <div class="card"><h3>BYOK encryption</h3><p>Pro tier: provide your public key. Herald encrypts with it and can never decrypt. You decrypt locally.</p></div>
    <div class="card"><h3>No payload inspection</h3><p>Herald is a pure relay. It never parses, inspects, routes on, or transforms your payload contents.</p></div>
  </div>
</section>

<section class="ecosystem max-w">
  <h2>Part of the stack</h2>
  <div class="eco-grid">
    <div class="eco-card"><div class="name accent">Herald</div><div class="role">The ears</div></div>
    <div class="eco-card"><a href="https://kindex.tools" style="color:var(--kindex)"><div class="name">Kindex</div></a><div class="role">The memory</div></div>
    <div class="eco-card"><div class="name" style="color:var(--fg)">Your agent</div><div class="role">The brain</div></div>
  </div>
</section>

<footer>
<div class="max-w">
  <div class="cols">
    <div>
      <h4>Sites</h4>
      <ul>
        <li><a href="https://exemplar.tools">exemplar.tools</a></li>
        <li><a href="https://signet.tools" class="signet">signet.tools</a></li>
        <li><a href="https://kindex.tools" class="kindex">kindex.tools</a></li>
        <li><a class="current">herald.tools</a></li>
        <li><a href="https://centaur.tools">centaur.tools</a></li>
        <li><a href="https://perardua.dev">perardua.dev</a></li>
      </ul>
    </div>
    <div>
      <h4>Links</h4>
      <ul>
        <li><a href="https://github.com/jmcentire/herald">GitHub</a></li>
        <li><a href="https://github.com/jmcentire/herald/blob/master/SPEC.md">Specification</a></li>
        <li><a href="https://github.com/jmcentire/herald/blob/master/LICENSE">License (MIT)</a></li>
      </ul>
    </div>
    <div>
      <h4>API</h4>
      <ul>
        <li><a href="https://proxy.herald.tools/health">Health check</a></li>
        <li><span style="font-size:.8125rem;color:var(--muted)">POST /:id/:endpoint</span></li>
        <li><span style="font-size:.8125rem;color:var(--muted)">GET /queue/:endpoint</span></li>
        <li><span style="font-size:.8125rem;color:var(--muted)">WS /stream/:endpoint</span></li>
      </ul>
    </div>
    <div>
      <h4>Author</h4>
      <ul>
        <li><span style="font-size:.8125rem;color:var(--fg)">Jeremy McEntire</span></li>
        <li><a href="https://perardua.dev">perardua.dev</a></li>
        <li><a href="https://github.com/jmcentire">GitHub</a></li>
      </ul>
    </div>
  </div>
  <div class="bottom">
    <span>Part of the <a href="https://exemplar.tools" style="color:var(--accent)">Exemplar</a> stack.</span>
    <span>MIT License</span>
  </div>
</div>
</footer>

</body>
</html>"##;
