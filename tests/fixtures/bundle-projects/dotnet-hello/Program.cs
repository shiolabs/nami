using System.Globalization;

// A German number needs ICU: without it .NET has no cultures but the invariant one.
var who = args.Length > 0 ? args[0] : "world";
var german = 1234.5.ToString("N1", new CultureInfo("de-DE"));
Console.WriteLine($"dotnet-hello 3.0.1: hello, {who} (.NET {Environment.Version.Major}, {german})");
