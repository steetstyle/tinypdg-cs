// Two methods, each with branching. The shape that hung: a file with more than one
// method produces a CFG that is several disjoint subgraphs concatenated, and the old
// dominator walk cycled between them.
namespace Agency.API;

public class MultiMethodEndpoint
{
    public static IResult First(int id, string? name)
    {
        if (id <= 0)
        {
            throw new ArgumentOutOfRangeException(nameof(id));
        }

        if (string.IsNullOrEmpty(name))
        {
            return Results.BadRequest("name required");
        }

        for (var i = 0; i < id; i++)
        {
            Console.WriteLine(i);
        }

        return Results.Ok(name);
    }

    public static IResult Second(Guid agencyId, int page)
    {
        if (page < 1)
        {
            return Results.BadRequest("page must be positive");
        }

        while (page > 100)
        {
            page /= 2;
        }

        try
        {
            return Results.Ok(Find(agencyId, page));
        }
        catch (InvalidOperationException)
        {
            return Results.Problem("lookup failed");
        }
        finally
        {
            Console.WriteLine("done");
        }
    }

    private static string Find(Guid id, int page) => $"{id}:{page}";
}
