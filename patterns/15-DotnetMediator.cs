using System;
using System.Collections.Generic;
using System.Threading.Tasks;

namespace MediatR.Conceptual
{
    public interface IRequest<TResponse>
    {
    }

    public interface IRequestHandler<TRequest, TResponse>
        where TRequest : IRequest<TResponse>
    {
        TResponse Handle(TRequest request);
    }

    public interface INotification
    {
    }

    public interface INotificationHandler<TNotification>
        where TNotification : INotification
    {
        void Handle(TNotification notification);
    }

    public interface IMediator
    {
        TResponse Send<TResponse>(IRequest<TResponse> request);
        void Publish<TNotification>(TNotification notification)
            where TNotification : INotification;
    }

    class GetUserQuery : IRequest<string>
    {
        public int UserId { get; set; }
    }

    class GetUserHandler : IRequestHandler<GetUserQuery, string>
    {
        public string Handle(GetUserQuery request)
        {
            return $"User {request.UserId}";
        }
    }

    class ListOrdersQuery : IRequest<string[]>
    {
    }

    class ListOrdersHandler : IRequestHandler<ListOrdersQuery, string[]>
    {
        public string[] Handle(ListOrdersQuery request)
        {
            return new[] { "Order1", "Order2" };
        }
    }

    class UserCreatedEvent : INotification
    {
        public int UserId { get; set; }
    }

    class EmailHandler : INotificationHandler<UserCreatedEvent>
    {
        public void Handle(UserCreatedEvent notification)
        {
            Console.WriteLine($"Sending email to user {notification.UserId}");
        }
    }

    class AuditHandler : INotificationHandler<UserCreatedEvent>
    {
        public void Handle(UserCreatedEvent notification)
        {
            Console.WriteLine($"Auditing user creation {notification.UserId}");
        }
    }

    class Mediator : IMediator
    {
        public TResponse Send<TResponse>(IRequest<TResponse> request)
        {
            Console.WriteLine($"Sending request of type {request.GetType().Name}");
            return default;
        }

        public void Publish<TNotification>(TNotification notification)
            where TNotification : INotification
        {
            Console.WriteLine($"Publishing notification {notification.GetType().Name}");
        }
    }

    class Program
    {
        static void Main(string[] args)
        {
            IMediator mediator = new Mediator();
            var user = mediator.Send(new GetUserQuery { UserId = 42 });
            mediator.Publish(new UserCreatedEvent { UserId = 42 });
        }
    }
}
