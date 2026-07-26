class C {
    void M() {
        for (int i = 0; i < 10; i++)
            System.Console.WriteLine(i);

        int[] arr = { 1, 2, 3 };
        foreach (var x in arr)
            System.Console.WriteLine(x);

        int j = 0;
        while (j < 10) {
            System.Console.WriteLine(j);
            j++;
        }

        int k = 0;
        do {
            System.Console.WriteLine(k);
            k++;
        } while (k < 10);
    }
}
