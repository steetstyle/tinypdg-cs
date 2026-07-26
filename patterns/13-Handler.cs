using System;

namespace RefactoringGuru.DesignPatterns.Handler.Conceptual
{
    public interface ICommandHandler
    {
        void Handle(object command);
    }

    public interface IQueryHandler
    {
        object Handle(object query);
    }

    class CreateOrderHandler : ICommandHandler
    {
        public void Handle(object command)
        {
            Console.WriteLine("CreateOrderHandler: Handling order creation.");
        }
    }

    class DeleteOrderHandler : ICommandHandler
    {
        public void Handle(object command)
        {
            Console.WriteLine("DeleteOrderHandler: Handling order deletion.");
        }
    }

    class GetOrderHandler : IQueryHandler
    {
        public object Handle(object query)
        {
            Console.WriteLine("GetOrderHandler: Returning order details.");
            return "Order details";
        }
    }

    class ListOrdersHandler : IQueryHandler
    {
        public object Handle(object query)
        {
            Console.WriteLine("ListOrdersHandler: Returning order list.");
            return new string[] { "Order1", "Order2" };
        }
    }

    class Program
    {
        static void Main(string[] args)
        {
            ICommandHandler create = new CreateOrderHandler();
            create.Handle("create_order");

            IQueryHandler get = new GetOrderHandler();
            get.Handle("get_order_42");
        }
    }
}
