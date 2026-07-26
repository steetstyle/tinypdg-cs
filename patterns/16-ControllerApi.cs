using System;
using System.Collections.Generic;
using System.Threading.Tasks;
using Microsoft.AspNetCore.Mvc;

namespace MyApi.Controllers
{
    public class UsersController : ControllerBase
    {
        [HttpGet]
        public IActionResult GetUsers()
        {
            return Ok(new[] { "Alice", "Bob" });
        }

        [HttpGet("{id}")]
        public IActionResult GetUser(int id)
        {
            return Ok($"User {id}");
        }

        [HttpPost]
        public IActionResult CreateUser([FromBody] string name)
        {
            return Ok($"Created {name}");
        }

        [HttpPut("{id}")]
        public IActionResult UpdateUser(int id, [FromBody] string name)
        {
            return Ok($"Updated {id}");
        }

        [HttpDelete("{id}")]
        public IActionResult DeleteUser(int id)
        {
            return Ok($"Deleted {id}");
        }
    }

    public class OrdersController : ControllerBase
    {
        [HttpGet]
        public IActionResult GetOrders()
        {
            return Ok(new[] { "Order1", "Order2" });
        }

        [HttpGet("{id}")]
        public IActionResult GetOrder(int id)
        {
            return Ok($"Order {id}");
        }

        [HttpPost]
        public IActionResult CreateOrder([FromBody] string item)
        {
            return Ok($"Created order for {item}");
        }
    }

    class Program
    {
        static void Main(string[] args)
        {
            Console.WriteLine("API running");
        }
    }
}
