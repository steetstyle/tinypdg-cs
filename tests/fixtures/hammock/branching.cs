// One method, nested branching, so the containment hierarchy has something to nest.
namespace Agency.API;

public class BranchingEndpoint
{
    public static IResult Handle(int id, string? kind)
    {
        if (id == 0)
        {
            return Results.BadRequest();
        }
        else if (id < 0)
        {
            throw new ArgumentException();
        }

        switch (kind)
        {
            case "a":
                return Results.Ok(1);
            case "b":
                return Results.Ok(2);
            default:
                break;
        }

        foreach (var item in Items())
        {
            if (item > 0)
            {
                Console.WriteLine(item);
            }
        }

        return Results.Ok();
    }

    private static int[] Items() => [1, 2, 3];
}
