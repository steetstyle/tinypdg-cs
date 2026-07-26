using System;
using System.Collections.Generic;
using Microsoft.AspNetCore.Builder;
using Microsoft.AspNetCore.Http;

namespace MyApi.Minimal
{
    public static class UserEndpoints
    {
        public static void MapUserEndpoints(this WebApplication app)
        {
            app.MapGet("/users", () => Results.Ok(new[] { "Alice", "Bob" }));
            app.MapGet("/users/{id}", (int id) => Results.Ok($"User {id}"));
            app.MapPost("/users", (string name) => Results.Ok($"Created {name}"));
        }
    }

    public static class OrderEndpoints
    {
        public static void MapOrderEndpoints(this WebApplication app)
        {
            app.MapGet("/orders", () => Results.Ok(new[] { "Order1", "Order2" }));
            app.MapPost("/orders", (string item) => Results.Ok($"Created order for {item}"));
        }
    }

    class Program
    {
        static void Main(string[] args)
        {
            var builder = WebApplication.CreateBuilder(args);
            var app = builder.Build();
            UserEndpoints.MapUserEndpoints(app);
            OrderEndpoints.MapOrderEndpoints(app);
            app.Run();
        }
    }
}
