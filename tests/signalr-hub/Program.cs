// Smallest ASP.NET Core SignalR hub that behaves like the Gecko hub described in
// dolusoft/gecko-project-backend#1974, for the Rust client trial (docs/rust-migration.md §7.4).
// It is a test fixture, not the real backend.
//
// Tokens (Authorization: Bearer <token>, never in the query string):
//   good    - normal connection
//   busy    - the server closes right after the handshake with Close error "4409 busy"
//   outer   - only accepted by /redirect/negotiate, which answers { url, accessToken: "good" }
//
// Settings (environment, milliseconds):
//   TEST_HUB_KEEPALIVE_MS       server Ping interval (default 15000)
//   TEST_HUB_CLIENT_TIMEOUT_MS  server drops a client that sent nothing for this long (default 30000)
//
// Run: dotnet run --urls http://127.0.0.1:5199

using System.Collections.Concurrent;
using System.Text.Json;
using Microsoft.AspNetCore.SignalR;
using Microsoft.AspNetCore.SignalR.Protocol;

var builder = WebApplication.CreateBuilder(args);
builder.Logging.SetMinimumLevel(LogLevel.Warning);

builder.Services
    .AddSignalR(options =>
    {
        options.KeepAliveInterval = TimeSpan.FromMilliseconds(EnvMs("TEST_HUB_KEEPALIVE_MS", 15000));
        options.ClientTimeoutInterval = TimeSpan.FromMilliseconds(EnvMs("TEST_HUB_CLIENT_TIMEOUT_MS", 30000));
        // #1974: capability parts reach 16 KB each; the hub must accept at least 64 KB.
        options.MaximumReceiveMessageSize = 64 * 1024;
        // A hub method that awaits a client result needs a second slot.
        options.MaximumParallelInvocationsPerClient = 2;
    });
builder.Services.AddSingleton<ConnectionRegistry>();
builder.Services.AddSingleton(typeof(HubLifetimeManager<>), typeof(TrackingLifetimeManager<>));

var app = builder.Build();
app.UseWebSockets();

string[] hubTokens = ["good", "busy"];

app.Use(async (context, next) =>
{
    var path = context.Request.Path;
    if (path.StartsWithSegments("/hub") || path.StartsWithSegments("/redirect"))
    {
        if (context.Request.Query.ContainsKey("access_token"))
        {
            await Reject(context, "token in the query string is not accepted");
            return;
        }

        var header = context.Request.Headers.Authorization.ToString();
        var token = header.StartsWith("Bearer ", StringComparison.Ordinal) ? header["Bearer ".Length..] : null;
        var allowed = path.StartsWithSegments("/redirect") ? token == "outer" : hubTokens.Contains(token);
        if (!allowed)
        {
            await Reject(context, "missing or unknown bearer token");
            return;
        }

        context.Items["token"] = token;
    }

    await next();
});

// Azure SignalR style negotiate redirect: the client must switch to `url` and `accessToken`.
app.MapPost("/redirect/negotiate", (HttpContext context) =>
{
    var request = context.Request;
    return Results.Json(new { url = $"{request.Scheme}://{request.Host}/hub", accessToken = "good" });
});

app.MapHub<TestHub>("/hub");

app.Lifetime.ApplicationStarted.Register(() => Console.WriteLine(
    $"ready keepAliveMs={EnvMs("TEST_HUB_KEEPALIVE_MS", 15000)} clientTimeoutMs={EnvMs("TEST_HUB_CLIENT_TIMEOUT_MS", 30000)}"));
app.Run();

static int EnvMs(string name, int fallback) =>
    int.TryParse(Environment.GetEnvironmentVariable(name), out var value) ? value : fallback;

static async Task Reject(HttpContext context, string message)
{
    context.Response.StatusCode = StatusCodes.Status401Unauthorized;
    await context.Response.WriteAsync(message);
}

public sealed class TestHub(ConnectionRegistry registry) : Hub
{
    public override async Task OnConnectedAsync()
    {
        if (Context.GetHttpContext()?.Items["token"] as string == "busy")
        {
            await registry.CloseAsync(Context.ConnectionId, "4409 busy");
            return;
        }

        await base.OnConnectedAsync();
    }

    // #1974: hub methods return { ok: true, value } or { ok: false, error: { code, ... } }.
    public object Status() => new { ok = true, value = new { bound = true, connectionId = Context.ConnectionId } };

    public async Task<object> Send(JsonElement envelope)
    {
        var kind = envelope.TryGetProperty("kind", out var value) && value.ValueKind == JsonValueKind.String
            ? value.GetString()
            : null;

        switch (kind)
        {
            case "refuse":
                return new { ok = false, error = new { code = "rate-limited", reason = "too many requests", retryAfterMs = 1000 } };

            case "throw":
                // Arrives as a Completion with `error`.
                throw new HubException("boom");

            case "close":
                var reason = envelope.TryGetProperty("reason", out var r) ? r.GetString() ?? "4409 busy" : "4409 busy";
                await registry.CloseAsync(Context.ConnectionId, reason);
                return new { ok = true, value = new { closed = true } };

            case "ask":
                // A server-to-client invocation that waits for a result. The CLI does not offer
                // client results, so it must answer with an error Completion and keep going.
                string answer;
                try
                {
                    answer = await Clients.Caller.InvokeAsync<string>("Question", "are you there", Context.ConnectionAborted);
                }
                catch (Exception error)
                {
                    answer = $"client error: {error.Message}";
                }

                return new { ok = true, value = new { answer } };

            case "burst":
                // Several records written back to back; the transport usually packs them in one frame.
                for (var i = 0; i < 3; i++)
                {
                    await Clients.Caller.SendAsync("Reply", new { kind = "burst", index = i });
                }

                return new { ok = true, value = new { sent = 3 } };

            default:
                await Clients.Caller.SendAsync("Reply", envelope);
                return new { ok = true, value = new { accepted = true } };
        }
    }
}

// Keeps the HubConnectionContext of every connection so the hub can write a Close message
// with a chosen error text; the public Abort() closes without one.
public sealed class ConnectionRegistry
{
    private readonly ConcurrentDictionary<string, HubConnectionContext> connections = new();

    public void Add(HubConnectionContext connection) => connections[connection.ConnectionId] = connection;

    public void Remove(HubConnectionContext connection) => connections.TryRemove(connection.ConnectionId, out _);

    public async Task CloseAsync(string connectionId, string error)
    {
        if (!connections.TryGetValue(connectionId, out var connection))
        {
            return;
        }

        await connection.WriteAsync(new CloseMessage(error, allowReconnect: false));
        connection.Abort();
    }
}

public sealed class TrackingLifetimeManager<THub>(ConnectionRegistry registry, ILogger<DefaultHubLifetimeManager<THub>> logger)
    : DefaultHubLifetimeManager<THub>(logger)
    where THub : Hub
{
    public override Task OnConnectedAsync(HubConnectionContext connection)
    {
        registry.Add(connection);
        return base.OnConnectedAsync(connection);
    }

    public override Task OnDisconnectedAsync(HubConnectionContext connection)
    {
        registry.Remove(connection);
        return base.OnDisconnectedAsync(connection);
    }
}
